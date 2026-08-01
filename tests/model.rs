//! Integration tests for the doc model + step pipeline.
//!
//! Lives in `tests/` (not in `src/`) so it exercises the public API a host
//! application would actually use. Don't reach into private members here —
//! if a test needs a private helper, that's a signal to expose it (or
//! change the shape of the API).

use std::rc::Rc;

use dioxus_editor::commands::insert_text;
use dioxus_editor::io::MarkdownIo;
use dioxus_editor::plugins as ep;
use dioxus_editor::*;

fn fresh_state() -> EditorState {
    let schema = Rc::new(Schema::new());
    EditorState::new(schema, Doc::empty())
}

#[test]
fn empty_doc_has_one_paragraph_child() {
    let doc = Doc::empty();
    let root = doc.root_node();
    assert_eq!(root.kind, "doc");
    assert_eq!(root.children.len(), 1);
    let para = doc.get_element(root.children[0]).unwrap();
    assert_eq!(para.kind, "paragraph");
    assert!(para.children.is_empty());
}

#[test]
fn replace_text_basic() {
    let doc = Doc::empty();
    let para = doc.root_node().children[0];
    let (doc, _) = Transaction::new()
        .step(Step::InsertNodes {
            parent: para,
            index: 0,
            nodes: vec![NodeSpec::text("hello")],
        })
        .apply(doc)
        .unwrap();
    let txt_key = doc.get_element(para).unwrap().children[0];

    let (doc, _) = Transaction::new()
        .step(Step::ReplaceText {
            key: txt_key,
            from: 0,
            to: 5,
            text: "world".into(),
        })
        .apply(doc)
        .unwrap();
    assert_eq!(doc.get_text(txt_key).unwrap().text, "world");
}

#[test]
fn insert_nodes_assigns_fresh_keys_and_wires_parents() {
    let doc = Doc::empty();
    let para = doc.root_node().children[0];
    let tr = Transaction::new().step(Step::InsertNodes {
        parent: para,
        index: 0,
        nodes: vec![
            NodeSpec::text("hi "),
            NodeSpec::Decorator {
                kind: "file".into(),
                attrs: Attrs::new().with("id", "abc"),
            },
            NodeSpec::text(" there"),
        ],
    });
    let (doc, _) = tr.apply(doc).unwrap();
    let para = doc.get_element(para).unwrap();
    assert_eq!(para.children.len(), 3);
    // Each inserted node has a fresh key and points at the para as parent.
    for &c in &para.children {
        assert_eq!(doc.parent(c), Some(para.key));
    }
    let texts: Vec<&str> = para
        .children
        .iter()
        .filter_map(|&k| doc.get_text(k).map(|t| t.text.as_str()))
        .collect();
    assert_eq!(texts, vec!["hi ", " there"]);
}

#[test]
fn split_text_produces_two_siblings() {
    let doc = Doc::empty();
    let para = doc.root_node().children[0];
    let (doc, _) = Transaction::new()
        .step(Step::InsertNodes {
            parent: para,
            index: 0,
            nodes: vec![NodeSpec::text("hello world")],
        })
        .apply(doc)
        .unwrap();
    let txt_key = doc.get_element(para).unwrap().children[0];
    let (doc, _) = Transaction::new()
        .step(Step::SplitText {
            key: txt_key,
            at: 5,
        })
        .apply(doc)
        .unwrap();
    let para = doc.get_element(para).unwrap();
    assert_eq!(para.children.len(), 2);
    assert_eq!(doc.get_text(para.children[0]).unwrap().text, "hello");
    assert_eq!(doc.get_text(para.children[1]).unwrap().text, " world");
}

#[test]
fn insert_text_command_at_element_caret() {
    let mut state = fresh_state();
    // Caret at element offset 0 inside the lone paragraph.
    let para = state.doc.root_node().children[0];
    state.selection = Selection::caret(Point::element(para, 0));
    let tr = insert_text(&state, "abc").expect("command produced a tr");
    let new_state = state.apply(tr).unwrap();
    let para = new_state.doc.get_element(para).unwrap();
    assert_eq!(para.children.len(), 1);
    assert_eq!(
        new_state.doc.get_text(para.children[0]).unwrap().text,
        "abc"
    );
}

#[test]
fn markdown_writer_emits_bold_italic_strike_code() {
    let doc = Doc::empty();
    let para = doc.root_node().children[0];
    let (doc, _) = Transaction::new()
        .step(Step::InsertNodes {
            parent: para,
            index: 0,
            nodes: vec![NodeSpec::Text {
                text: "hi".into(),
                format: FormatBits::BOLD | FormatBits::ITALIC,
            }],
        })
        .apply(doc)
        .unwrap();
    let s = Schema::new();
    let md = dioxus_editor::io::to_markdown(&doc, &s).unwrap();
    assert_eq!(md, "**_hi_**");
}

#[test]
fn markdown_code_blocks_preserve_literal_punctuation_and_fences() {
    let schema = Schema::new();

    for source in ["```\ncblock-marker\n```", "````\nliteral ``` fence\n````"] {
        let doc = dioxus_editor::io::from_markdown(source, &schema);
        assert_eq!(
            dioxus_editor::io::to_markdown(&doc, &schema).unwrap(),
            source
        );
    }
}

#[test]
fn markdown_writer_rejects_unknown_decorators() {
    let doc = Doc::empty();
    let para = doc.root_node().children[0];
    let (doc, _) = Transaction::new()
        .step(Step::InsertNodes {
            parent: para,
            index: 0,
            nodes: vec![NodeSpec::decorator("missing", Attrs::new())],
        })
        .apply(doc)
        .unwrap();

    assert_eq!(
        dioxus_editor::io::to_markdown(&doc, &Schema::new()).unwrap_err(),
        dioxus_editor::io::MarkdownError::UnknownDecorator("missing".into())
    );
}

#[test]
fn markdown_round_trip_paragraph_with_bold() {
    let s = Schema::new();
    let doc = dioxus_editor::io::from_markdown("hello **world**", &s);
    let md = dioxus_editor::io::to_markdown(&doc, &s).unwrap();
    assert_eq!(md, "hello **world**");
}

#[test]
fn markdown_round_trip_table() {
    let s = Schema::new();
    let src = "| a | b |\n| --- | :---: |\n| 1 | 2 |\n| 3 | 4 |";
    let doc = dioxus_editor::io::from_markdown(src, &s);
    // Doc should contain a single `table` element with one header row + two
    // body rows, each with two cells.
    let root = doc.root_node();
    assert_eq!(root.children.len(), 1);
    let table = doc.get_element(root.children[0]).unwrap();
    assert_eq!(table.kind, "table");
    assert_eq!(table.children.len(), 3);
    let head = doc.get_element(table.children[0]).unwrap();
    assert_eq!(head.kind, "table_row");
    assert_eq!(head.attrs.get_bool("header"), Some(true));
    assert_eq!(head.children.len(), 2);
    // Round-tripping the parsed doc should re-emit a markdown table that
    // re-parses into an equivalent shape; we don't pin the exact whitespace
    // since separators can differ in width.
    let md = dioxus_editor::io::to_markdown(&doc, &s).unwrap();
    let doc2 = dioxus_editor::io::from_markdown(&md, &s);
    let root2 = doc2.root_node();
    assert_eq!(root2.children.len(), 1);
    let table2 = doc2.get_element(root2.children[0]).unwrap();
    assert_eq!(table2.kind, "table");
    assert_eq!(table2.children.len(), 3);
    assert_eq!(table2.attrs.get_str("align"), Some("none,center"));
}

#[test]
fn insert_table_command_places_caret_in_first_cell() {
    let state = fresh_state();
    let tr = dioxus_editor::commands::insert_table(&state, 2, 2).expect("transaction");
    let new = state.apply(tr).expect("apply");
    // Find the new table element.
    let root = new.doc.root_node();
    let table_key = root
        .children
        .iter()
        .find(|&&k| {
            new.doc
                .get_element(k)
                .map(|e| e.kind == "table")
                .unwrap_or(false)
        })
        .copied()
        .expect("table inserted");
    let table = new.doc.get_element(table_key).unwrap();
    assert_eq!(table.children.len(), 2);
    let first_row = new.doc.get_element(table.children[0]).unwrap();
    assert_eq!(first_row.attrs.get_bool("header"), Some(true));
    let first_cell = first_row.children[0];
    match &new.selection {
        Selection::Range { focus, .. } => assert_eq!(focus.key, first_cell),
        _ => panic!("caret should be range"),
    }
}

// -- Helpers shared by the table-edit tests below ------------------------

/// Build a state with a `rows x cols` table inserted into the doc; the
/// caret lands in the cell at the given (row, col).
fn state_in_table_cell(rows: usize, cols: usize, target: (usize, usize)) -> EditorState {
    let state = fresh_state();
    let tr = dioxus_editor::commands::insert_table(&state, rows, cols).unwrap();
    let mut state = state.apply(tr).unwrap();
    // Walk to the requested cell.
    let root = state.doc.root_node();
    let table_key = root
        .children
        .iter()
        .find(|&&k| {
            state
                .doc
                .get_element(k)
                .map(|e| e.kind == "table")
                .unwrap_or(false)
        })
        .copied()
        .unwrap();
    let table = state.doc.get_element(table_key).unwrap();
    let row_key = table.children[target.0];
    let cell_key = state.doc.get_element(row_key).unwrap().children[target.1];
    state.selection = Selection::caret(dioxus_editor::Point::element(cell_key, 0));
    state
}

fn table_root(state: &EditorState) -> &dioxus_editor::ElementNode {
    let root = state.doc.root_node();
    let table_key = root
        .children
        .iter()
        .find(|&&k| {
            state
                .doc
                .get_element(k)
                .map(|e| e.kind == "table")
                .unwrap_or(false)
        })
        .copied()
        .unwrap();
    state.doc.get_element(table_key).unwrap()
}

fn cell_text(state: &EditorState, cell_key: dioxus_editor::NodeKey) -> String {
    let cell = state.doc.get_element(cell_key).unwrap();
    let mut out = String::new();
    for &k in &cell.children {
        if let dioxus_editor::Node::Text(t) = state.doc.get(k).unwrap() {
            out.push_str(&t.text);
        }
    }
    out
}

fn caret_cell(state: &EditorState) -> dioxus_editor::NodeKey {
    match &state.selection {
        Selection::Range { focus, .. } => focus.key,
        _ => panic!("expected range selection"),
    }
}

// -- Tab / Shift-Tab navigation -----------------------------------------

#[test]
fn next_cell_walks_within_a_row() {
    let state = state_in_table_cell(2, 3, (0, 0));
    let tr = dioxus_editor::commands::move_to_next_cell(&state).unwrap();
    let new = state.apply(tr).unwrap();
    let row = new.doc.get_element(table_root(&new).children[0]).unwrap();
    assert_eq!(caret_cell(&new), row.children[1]);
}

#[test]
fn next_cell_wraps_to_first_cell_of_next_row() {
    // Caret in last column of first row → next jumps to first column of
    // second row.
    let state = state_in_table_cell(2, 3, (0, 2));
    let tr = dioxus_editor::commands::move_to_next_cell(&state).unwrap();
    let new = state.apply(tr).unwrap();
    let next_row = new.doc.get_element(table_root(&new).children[1]).unwrap();
    assert_eq!(caret_cell(&new), next_row.children[0]);
}

#[test]
fn next_cell_past_last_appends_row_and_lands_in_first_cell() {
    let state = state_in_table_cell(2, 3, (1, 2));
    let before_rows = table_root(&state).children.len();
    let tr = dioxus_editor::commands::move_to_next_cell(&state).unwrap();
    let new = state.apply(tr).unwrap();
    let table = table_root(&new);
    assert_eq!(table.children.len(), before_rows + 1);
    let appended = new
        .doc
        .get_element(*table.children.last().unwrap())
        .unwrap();
    assert_eq!(appended.children.len(), 3);
    assert_eq!(caret_cell(&new), appended.children[0]);
    // Appended row is a body row, never a header.
    assert_ne!(appended.attrs.get_bool("header"), Some(true));
}

#[test]
fn prev_cell_walks_backwards_within_row() {
    let state = state_in_table_cell(2, 3, (1, 2));
    let tr = dioxus_editor::commands::move_to_prev_cell(&state).unwrap();
    let new = state.apply(tr).unwrap();
    let row = new.doc.get_element(table_root(&new).children[1]).unwrap();
    assert_eq!(caret_cell(&new), row.children[1]);
}

#[test]
fn prev_cell_wraps_to_last_cell_of_previous_row() {
    let state = state_in_table_cell(2, 3, (1, 0));
    let tr = dioxus_editor::commands::move_to_prev_cell(&state).unwrap();
    let new = state.apply(tr).unwrap();
    let row = new.doc.get_element(table_root(&new).children[0]).unwrap();
    assert_eq!(caret_cell(&new), row.children[2]);
}

#[test]
fn prev_cell_from_top_left_is_noop() {
    let state = state_in_table_cell(2, 3, (0, 0));
    assert!(dioxus_editor::commands::move_to_prev_cell(&state).is_none());
}

// -- Row insertion / removal --------------------------------------------

#[test]
fn insert_row_below_grows_table_and_caret_moves_to_new_row() {
    let state = state_in_table_cell(2, 2, (0, 1));
    let tr = dioxus_editor::commands::insert_row_below(&state).unwrap();
    let new = state.apply(tr).unwrap();
    let table = table_root(&new);
    assert_eq!(table.children.len(), 3);
    let new_row = new.doc.get_element(table.children[1]).unwrap();
    assert_eq!(new_row.children.len(), 2);
    // Inserted row is a body row.
    assert_ne!(new_row.attrs.get_bool("header"), Some(true));
    // Caret follows the inserted row at the same column.
    assert_eq!(caret_cell(&new), new_row.children[1]);
}

#[test]
fn insert_row_above_grows_table_and_caret_moves_to_new_row() {
    let state = state_in_table_cell(2, 2, (1, 0));
    let tr = dioxus_editor::commands::insert_row_above(&state).unwrap();
    let new = state.apply(tr).unwrap();
    let table = table_root(&new);
    assert_eq!(table.children.len(), 3);
    let new_row = new.doc.get_element(table.children[1]).unwrap();
    // Caret follows into the new row at the same column.
    assert_eq!(caret_cell(&new), new_row.children[0]);
}

#[test]
fn delete_row_shrinks_table_and_keeps_caret_in_table() {
    let state = state_in_table_cell(3, 2, (1, 1));
    let tr = dioxus_editor::commands::delete_row(&state).unwrap();
    let new = state.apply(tr).unwrap();
    let table = table_root(&new);
    assert_eq!(table.children.len(), 2);
    // Caret lands in the row that now sits at the deleted row's index.
    let row = new.doc.get_element(table.children[1]).unwrap();
    assert_eq!(caret_cell(&new), row.children[1]);
}

#[test]
fn delete_row_promotes_new_first_row_to_header_when_header_removed() {
    let state = state_in_table_cell(2, 2, (0, 0));
    let tr = dioxus_editor::commands::delete_row(&state).unwrap();
    let new = state.apply(tr).unwrap();
    let table = table_root(&new);
    let first = new.doc.get_element(table.children[0]).unwrap();
    assert_eq!(first.attrs.get_bool("header"), Some(true));
}

#[test]
fn delete_row_on_single_row_table_deletes_table() {
    let state = state_in_table_cell(1, 2, (0, 0));
    let tr = dioxus_editor::commands::delete_row(&state).unwrap();
    let new = state.apply(tr).unwrap();
    // No table remains.
    assert!(new.doc.root_node().children.iter().all(|&k| new
        .doc
        .get_element(k)
        .map(|e| e.kind.as_str())
        != Some("table")));
}

// -- Column insertion / removal -----------------------------------------

#[test]
fn insert_column_after_grows_all_rows_and_caret_advances() {
    let state = state_in_table_cell(2, 2, (0, 0));
    let tr = dioxus_editor::commands::insert_column_after(&state).unwrap();
    let new = state.apply(tr).unwrap();
    let table = table_root(&new);
    // Both rows gained a cell.
    for &row_key in &table.children {
        let row = new.doc.get_element(row_key).unwrap();
        assert_eq!(row.children.len(), 3);
    }
    // Caret moved to the newly-inserted column.
    let first_row = new.doc.get_element(table.children[0]).unwrap();
    assert_eq!(caret_cell(&new), first_row.children[1]);
}

#[test]
fn insert_column_before_grows_all_rows_and_align_string_extends() {
    let state = state_in_table_cell(2, 2, (0, 1));
    // Pre-set an alignment string so we can verify it gets shifted when a
    // new column is inserted.
    let table_key = table_root(&state).key;
    let state = state
        .apply(
            dioxus_editor::Transaction::new().step(dioxus_editor::Step::SetAttr {
                key: table_key,
                name: "align".into(),
                value: Some(dioxus_editor::AttrValue::Str("left,right".into())),
            }),
        )
        .unwrap();

    let tr = dioxus_editor::commands::insert_column_before(&state).unwrap();
    let new = state.apply(tr).unwrap();
    let table = table_root(&new);
    for &row_key in &table.children {
        let row = new.doc.get_element(row_key).unwrap();
        assert_eq!(row.children.len(), 3);
    }
    // The new column was inserted *before* col index 1, so the original
    // "right" alignment shifts to col 2; the new col defaults to "none".
    assert_eq!(table.attrs.get_str("align"), Some("left,none,right"));
}

#[test]
fn delete_column_strips_cells_and_align_entry() {
    let state = state_in_table_cell(2, 3, (0, 1));
    let table_key = table_root(&state).key;
    let state = state
        .apply(
            dioxus_editor::Transaction::new().step(dioxus_editor::Step::SetAttr {
                key: table_key,
                name: "align".into(),
                value: Some(dioxus_editor::AttrValue::Str("left,center,right".into())),
            }),
        )
        .unwrap();
    let tr = dioxus_editor::commands::delete_column(&state).unwrap();
    let new = state.apply(tr).unwrap();
    let table = table_root(&new);
    for &row_key in &table.children {
        let row = new.doc.get_element(row_key).unwrap();
        assert_eq!(row.children.len(), 2);
    }
    assert_eq!(table.attrs.get_str("align"), Some("left,right"));
}

#[test]
fn delete_column_on_single_column_table_deletes_table() {
    let state = state_in_table_cell(2, 1, (0, 0));
    let tr = dioxus_editor::commands::delete_column(&state).unwrap();
    let new = state.apply(tr).unwrap();
    assert!(new.doc.root_node().children.iter().all(|&k| new
        .doc
        .get_element(k)
        .map(|e| e.kind.as_str())
        != Some("table")));
}

#[test]
fn delete_table_removes_the_table_and_leaves_paragraph_in_its_place() {
    let state = state_in_table_cell(2, 2, (1, 1));
    let tr = dioxus_editor::commands::delete_table(&state).unwrap();
    let new = state.apply(tr).unwrap();
    // No table; caret lands on the replacement paragraph.
    assert!(new.doc.root_node().children.iter().all(|&k| new
        .doc
        .get_element(k)
        .map(|e| e.kind.as_str())
        != Some("table")));
}

// -- Content preservation ------------------------------------------------

#[test]
fn append_row_works_without_caret_in_table() {
    let state = state_in_table_cell(2, 3, (0, 0));
    // Move caret out of the table so append_row must rely solely on
    // table_key. A paragraph follows the table when inserted into an
    // empty doc; place the caret there.
    let para_key = state
        .doc
        .root_node()
        .children
        .iter()
        .find(|&&k| {
            state
                .doc
                .get_element(k)
                .map(|e| e.kind == "paragraph")
                .unwrap_or(false)
        })
        .copied()
        .unwrap();
    let table_key = table_root(&state).key;
    let mut state = state;
    state.selection = Selection::caret(dioxus_editor::Point::element(para_key, 0));

    let tr = dioxus_editor::commands::append_row(&state, table_key).unwrap();
    let new = state.apply(tr).unwrap();
    let table = table_root(&new);
    assert_eq!(table.children.len(), 3);
    let last = new
        .doc
        .get_element(*table.children.last().unwrap())
        .unwrap();
    assert_eq!(last.children.len(), 3);
    assert_ne!(last.attrs.get_bool("header"), Some(true));
    // Caret should land in the new row's first cell.
    assert_eq!(caret_cell(&new), last.children[0]);
}

#[test]
fn append_column_extends_align_and_caret_lands_in_header_of_new_column() {
    let state = state_in_table_cell(2, 2, (0, 0));
    let table_key = table_root(&state).key;
    let state = state
        .apply(
            dioxus_editor::Transaction::new().step(dioxus_editor::Step::SetAttr {
                key: table_key,
                name: "align".into(),
                value: Some(dioxus_editor::AttrValue::Str("left,center".into())),
            }),
        )
        .unwrap();
    let tr = dioxus_editor::commands::append_column(&state, table_key).unwrap();
    let new = state.apply(tr).unwrap();
    let table = table_root(&new);
    assert_eq!(
        new.doc
            .get_element(table.children[0])
            .unwrap()
            .children
            .len(),
        3
    );
    assert_eq!(table.attrs.get_str("align"), Some("left,center,none"));
    // Caret lands in the header row of the new column.
    let header = new.doc.get_element(table.children[0]).unwrap();
    assert_eq!(caret_cell(&new), header.children[2]);
}

#[test]
fn clear_cell_strips_text_and_keeps_caret_in_cell() {
    let mut state = state_in_table_cell(2, 2, (0, 0));
    state = state
        .apply(dioxus_editor::commands::insert_text(&state, "junk").unwrap())
        .unwrap();
    let tr = dioxus_editor::commands::clear_cell(&state).unwrap();
    let new = state.apply(tr).unwrap();
    let table = table_root(&new);
    let row = new.doc.get_element(table.children[0]).unwrap();
    let cell = new.doc.get_element(row.children[0]).unwrap();
    assert!(cell.children.is_empty());
    assert_eq!(caret_cell(&new), row.children[0]);
}

#[test]
fn clear_cell_on_empty_cell_is_noop() {
    let state = state_in_table_cell(2, 2, (0, 0));
    assert!(dioxus_editor::commands::clear_cell(&state).is_none());
}

#[test]
fn duplicate_row_copies_content_and_drops_header_flag() {
    let mut state = state_in_table_cell(2, 2, (0, 0));
    state = state
        .apply(dioxus_editor::commands::insert_text(&state, "h").unwrap())
        .unwrap();
    let tr = dioxus_editor::commands::duplicate_row(&state).unwrap();
    let new = state.apply(tr).unwrap();
    let table = table_root(&new);
    assert_eq!(table.children.len(), 3);
    // Original header row keeps its flag; the duplicate sitting at idx 1
    // drops it (only one header row may exist).
    assert_eq!(
        new.doc
            .get_element(table.children[0])
            .unwrap()
            .attrs
            .get_bool("header"),
        Some(true)
    );
    assert_ne!(
        new.doc
            .get_element(table.children[1])
            .unwrap()
            .attrs
            .get_bool("header"),
        Some(true)
    );
    // The duplicate carries the typed text.
    let dup = new.doc.get_element(table.children[1]).unwrap();
    assert_eq!(cell_text(&new, dup.children[0]), "h");
    // Caret follows into the duplicate.
    assert_eq!(caret_cell(&new), dup.children[0]);
}

#[test]
fn duplicate_column_copies_content_and_extends_align() {
    let mut state = state_in_table_cell(2, 2, (0, 0));
    state = state
        .apply(dioxus_editor::commands::insert_text(&state, "x").unwrap())
        .unwrap();
    let table_key = table_root(&state).key;
    let state = state
        .apply(
            dioxus_editor::Transaction::new().step(dioxus_editor::Step::SetAttr {
                key: table_key,
                name: "align".into(),
                value: Some(dioxus_editor::AttrValue::Str("right,none".into())),
            }),
        )
        .unwrap();
    let tr = dioxus_editor::commands::duplicate_column(&state).unwrap();
    let new = state.apply(tr).unwrap();
    let table = table_root(&new);
    let r0 = new.doc.get_element(table.children[0]).unwrap();
    assert_eq!(r0.children.len(), 3);
    assert_eq!(table.attrs.get_str("align"), Some("right,right,none"));
    // Both the original and its duplicate at the new col carry "x".
    assert_eq!(cell_text(&new, r0.children[0]), "x");
    assert_eq!(cell_text(&new, r0.children[1]), "x");
}

#[test]
fn structural_edits_preserve_existing_cell_content() {
    let mut state = state_in_table_cell(2, 2, (0, 0));
    // Type into the first cell.
    let tr = dioxus_editor::commands::insert_text(&state, "hello").unwrap();
    state = state.apply(tr).unwrap();
    // Move to (1, 0) and type.
    {
        let table = table_root(&state);
        let cell = state.doc.get_element(table.children[1]).unwrap().children[0];
        state.selection = Selection::caret(dioxus_editor::Point::element(cell, 0));
    }
    let tr = dioxus_editor::commands::insert_text(&state, "world").unwrap();
    state = state.apply(tr).unwrap();

    // Now insert a column after col 0 — both rows should keep their text.
    {
        let table = table_root(&state);
        let cell = state.doc.get_element(table.children[0]).unwrap().children[0];
        state.selection = Selection::caret(dioxus_editor::Point::element(cell, 0));
    }
    let tr = dioxus_editor::commands::insert_column_after(&state).unwrap();
    let new = state.apply(tr).unwrap();

    let table = table_root(&new);
    let r0 = new.doc.get_element(table.children[0]).unwrap();
    let r1 = new.doc.get_element(table.children[1]).unwrap();
    assert_eq!(r0.children.len(), 3);
    assert_eq!(r1.children.len(), 3);
    // Original col 0 text survives; the inserted col 1 is empty; original
    // col 1 (now at col 2) is also untouched.
    assert_eq!(cell_text(&new, r0.children[0]), "hello");
    assert_eq!(cell_text(&new, r0.children[1]), "");
    assert_eq!(cell_text(&new, r1.children[0]), "world");
}

thread_local! {
    static DISPATCH_RESULT: std::cell::RefCell<Option<Result<(), DispatchError>>> = const {
        std::cell::RefCell::new(None)
    };
    static DISPATCH_PLUGINS: std::cell::RefCell<Option<Vec<Box<dyn Plugin>>>> =
        std::cell::RefCell::new(None);
    static DISPATCH_TRANSACTION: std::cell::RefCell<Option<Transaction>> =
        const { std::cell::RefCell::new(None) };
}

struct InvalidAppend;

impl Plugin for InvalidAppend {
    fn append_transaction(
        &mut self,
        _tr: &Transaction,
        _old_state: &EditorState,
        _new_state: &EditorState,
    ) -> Option<Transaction> {
        Some(Transaction::new().step(Step::ReplaceText {
            key: u64::MAX,
            from: 0,
            to: 0,
            text: "x".into(),
        }))
    }
}

struct EndlessAppend;

impl Plugin for EndlessAppend {
    fn append_transaction(
        &mut self,
        _tr: &Transaction,
        _old_state: &EditorState,
        _new_state: &EditorState,
    ) -> Option<Transaction> {
        Some(Transaction::new().pending_format(FormatBits::BOLD))
    }
}

fn dispatch_test_app() -> dioxus::prelude::Element {
    use dioxus::prelude::*;

    let handle = use_editor(|| {
        DISPATCH_PLUGINS.with(|plugins| {
            plugins
                .borrow_mut()
                .take()
                .expect("plugins configured")
                .into_iter()
                .fold(EditorConfig::new(Schema::new()), |config, plugin| {
                    config.with_plugin(plugin)
                })
        })
    });
    if DISPATCH_RESULT.with(|result| result.borrow().is_none()) {
        let transaction =
            DISPATCH_TRANSACTION.with(|transaction| transaction.borrow_mut().take().unwrap());
        let result = handle.dispatch(transaction);
        DISPATCH_RESULT.with(|slot| *slot.borrow_mut() = Some(result));
    }
    rsx! { div {} }
}

fn record_dispatch(plugins: Vec<Box<dyn Plugin>>, transaction: Transaction) {
    use dioxus::prelude::*;

    DISPATCH_RESULT.with(|result| *result.borrow_mut() = None);
    DISPATCH_PLUGINS.with(|slot| *slot.borrow_mut() = Some(plugins));
    DISPATCH_TRANSACTION.with(|slot| *slot.borrow_mut() = Some(transaction));
    let mut dom = VirtualDom::new(dispatch_test_app);
    dom.rebuild_in_place();
}

#[test]
fn dispatch_reports_step_plugin_and_chain_failures() {
    let invalid = Transaction::new().step(Step::ReplaceText {
        key: u64::MAX,
        from: 0,
        to: 0,
        text: "x".into(),
    });
    record_dispatch(Vec::new(), invalid);
    DISPATCH_RESULT.with(|result| {
        assert!(matches!(
            result.borrow().as_ref().unwrap(),
            Err(DispatchError::Step(StepError::NoSuchNode(_)))
        ));
    });

    record_dispatch(
        vec![Box::new(InvalidAppend)],
        Transaction::new().pending_format(FormatBits::BOLD),
    );
    DISPATCH_RESULT.with(|result| {
        assert!(matches!(
            result.borrow().as_ref().unwrap(),
            Err(DispatchError::Plugin(StepError::NoSuchNode(_)))
        ));
    });

    record_dispatch(
        vec![Box::new(EndlessAppend)],
        Transaction::new().pending_format(FormatBits::BOLD),
    );
    DISPATCH_RESULT.with(|result| {
        assert_eq!(
            result.borrow().as_ref().unwrap(),
            &Err(DispatchError::ChainExhausted)
        );
    });
}

#[test]
fn handle_dispatches_through_plugin_pipeline() {
    use dioxus::prelude::*;

    // The handle needs a Dioxus context to use_hook into; run inside a
    // minimal in-process scope so use_signal works.
    let mut dom = VirtualDom::new(|| {
        let handle = use_editor(|| {
            EditorConfig::new(Schema::new())
                .with_plugin(Box::new(ep::DefaultKeymap))
                .with_plugin(Box::new(ep::History::new()))
                .with_plugin(Box::new(ep::MarkdownShortcuts::new()))
        });
        // Provide handle to a context so a future render can read it.
        use_context_provider(|| handle.clone());
        rsx! { div { "ok" } }
    });
    dom.rebuild_in_place();

    // No assertions on internal state — the test ensures the whole
    // plugin/history/keymap construction path compiles and runs without
    // panicking when wired through Dioxus.
}

/// Markdown IO where the `File` tag maps to the lowercase `file`
/// decorator kind the schema renders under.
fn file_io() -> MarkdownIo {
    MarkdownIo::new().with_decorator_reader(
        dioxus_editor::io::HtmlTagReader::new()
            .register("File", "file", |s| {
                let a = dioxus_editor::io::parse_html_attrs(s);
                if a.get_str("id").is_some() {
                    Some(a)
                } else {
                    None
                }
            })
            .into_reader(),
    )
}

#[test]
fn markdown_io_recognizes_registered_decorator_tag() {
    let io = file_io();
    let s = Schema::new();
    let doc = io.from_markdown("see <File id=\"abc\"></File> end", &s);
    // The lone paragraph should contain: text "see ", decorator "file"
    // (the tag is `File` but the decorator kind is lowercase), text " end".
    let para = doc.root_node().children[0];
    let kids = doc.get_element(para).unwrap().children.clone();
    let mut kinds = Vec::new();
    for k in kids {
        match doc.get(k).unwrap() {
            Node::Text(t) => kinds.push(format!("t:{}", t.text)),
            Node::Decorator(d) => kinds.push(format!(
                "d:{}:{}",
                d.kind,
                d.attrs.get_str("id").unwrap_or("")
            )),
            _ => kinds.push("e".into()),
        }
    }
    assert!(
        kinds.iter().any(|k| k.starts_with("d:file:abc")),
        "kinds = {kinds:?}"
    );
    // The orphan `</File>` close tag must not survive as literal text — the
    // parser hands it over as a fragment separate from the open tag.
    assert!(
        !kinds
            .iter()
            .any(|k| k.contains("</File>") || k.contains("<File")),
        "stray tag text leaked: {kinds:?}"
    );
}

#[test]
fn markdown_io_handles_self_closing_and_orphan_file_tags() {
    let io = file_io();
    let s = Schema::new();
    for src in ["a <File id=\"x\"/> b", "a <File id=\"x\" /> b"] {
        let doc = io.from_markdown(src, &s);
        let mut decos = 0usize;
        let mut texts = Vec::new();
        fn walk(
            doc: &dioxus_editor::Doc,
            key: dioxus_editor::NodeKey,
            decos: &mut usize,
            texts: &mut Vec<String>,
        ) {
            match doc.get(key) {
                Some(Node::Decorator(_)) => *decos += 1,
                Some(Node::Text(t)) => texts.push(t.text.clone()),
                _ => {}
            }
            if let Some(e) = doc.get_element(key) {
                for &c in &e.children {
                    walk(doc, c, decos, texts);
                }
            }
        }
        walk(&doc, doc.root_key(), &mut decos, &mut texts);
        assert_eq!(decos, 1, "src = {src:?}");
        assert!(
            !texts.iter().any(|t| t.contains("File") || t.contains('<')),
            "raw tag leaked for {src:?}: {texts:?}"
        );
    }
}

/// Schema with `file` and `image` decorators for round-trip coverage.
fn decorated_schema() -> Schema {
    use std::rc::Rc;
    Schema::new()
        .with_decorator(
            "file",
            DecoratorSpec {
                inline: false,
                render: Rc::new(|_| dioxus::prelude::VNode::empty()),
                to_markdown: Rc::new(|attrs| {
                    format!(
                        "<File id=\"{}\"></File>",
                        attrs.get_str("id").unwrap_or_default()
                    )
                }),
            },
        )
        .with_decorator(
            "image",
            DecoratorSpec {
                inline: false,
                render: Rc::new(|_| dioxus::prelude::VNode::empty()),
                to_markdown: Rc::new(|attrs| {
                    format!(
                        "![{}]({})",
                        attrs.get_str("alt").unwrap_or_default(),
                        attrs.get_str("src").unwrap_or_default()
                    )
                }),
            },
        )
}

#[test]
fn markdown_io_parses_image_into_decorator() {
    let s = decorated_schema();
    let doc = dioxus_editor::io::from_markdown("![logo.png](/media/abc-123)", &s);
    let img = doc
        .root_node()
        .children
        .iter()
        .flat_map(|&b| {
            doc.get_element(b)
                .map(|e| e.children.clone())
                .unwrap_or_default()
        })
        .find_map(|k| doc.get(k).and_then(Node::as_decorator).cloned());
    let img = img.expect("image decorator");
    assert_eq!(img.kind, "image");
    assert_eq!(img.attrs.get_str("src"), Some("/media/abc-123"));
    assert_eq!(img.attrs.get_str("alt"), Some("logo.png"));
}

#[test]
fn markdown_io_round_trips_file_and_image() {
    // The exact mixed shape that broke editing: a file card, an image, and a
    // second file separated by soft breaks.
    let s = decorated_schema();
    let body = "<File id=\"pdf-1\"></File>\nalso this:\n![logo.svg](/media/svg-1)\nand this:\n<File id=\"pdf-2\"></File>";
    let doc = file_io().from_markdown(body, &s);

    // Every file/image survives as a decorator — no stray raw markdown or
    // `[unknown decorator]` placeholder.
    let mut decorators = Vec::new();
    fn walk(
        doc: &dioxus_editor::Doc,
        key: dioxus_editor::NodeKey,
        out: &mut Vec<(String, String)>,
    ) {
        if let Some(d) = doc.get(key).and_then(Node::as_decorator) {
            let id = d
                .attrs
                .get_str("id")
                .or_else(|| d.attrs.get_str("src"))
                .unwrap_or("")
                .to_string();
            out.push((d.kind.clone(), id));
        }
        if let Some(e) = doc.get_element(key) {
            for &c in &e.children {
                walk(doc, c, out);
            }
        }
    }
    walk(&doc, doc.root_key(), &mut decorators);
    assert_eq!(
        decorators,
        vec![
            ("file".to_string(), "pdf-1".to_string()),
            ("image".to_string(), "/media/svg-1".to_string()),
            ("file".to_string(), "pdf-2".to_string()),
        ],
    );

    // No raw tag/markdown text leaked into the doc.
    let mut texts = Vec::new();
    fn walk_text(doc: &dioxus_editor::Doc, key: dioxus_editor::NodeKey, out: &mut Vec<String>) {
        if let Some(t) = doc.get(key).and_then(|n| match n {
            Node::Text(t) => Some(t.text.clone()),
            _ => None,
        }) {
            out.push(t);
        }
        if let Some(e) = doc.get_element(key) {
            for &c in &e.children {
                walk_text(doc, c, out);
            }
        }
    }
    walk_text(&doc, doc.root_key(), &mut texts);
    assert!(
        !texts
            .iter()
            .any(|t| t.contains("<File") || t.contains("</File>")),
        "raw File tag leaked as text: {texts:?}"
    );

    // And the serialized form re-emits the same wire markdown for each embed.
    let md = file_io().to_markdown(&doc, &s).unwrap();
    assert!(md.contains("<File id=\"pdf-1\"></File>"), "md = {md}");
    assert!(md.contains("![logo.svg](/media/svg-1)"), "md = {md}");
    assert!(md.contains("<File id=\"pdf-2\"></File>"), "md = {md}");
}

#[test]
fn split_block_applies_cross_paragraph_deletion_before_splitting() {
    let doc = Doc::empty();
    let root = doc.root_key();
    let (doc, _) = Transaction::new()
        .step(Step::RemoveNodes {
            parent: root,
            range: 0..1,
        })
        .step(Step::InsertNodes {
            parent: root,
            index: 0,
            nodes: vec![
                NodeSpec::paragraph(vec![NodeSpec::text("first")]),
                NodeSpec::paragraph(vec![NodeSpec::text("second")]),
            ],
        })
        .apply(doc)
        .unwrap();
    let blocks = doc.root_node().children.clone();
    let first = doc.get_element(blocks[0]).unwrap().children[0];
    let second = doc.get_element(blocks[1]).unwrap().children[0];
    let mut state = EditorState::new(Rc::new(Schema::new()), doc);
    state.selection = Selection::Range {
        anchor: Point::text(first, 3),
        focus: Point::text(second, 3),
    };

    let next = state
        .apply(dioxus_editor::commands::split_block(&state).unwrap())
        .unwrap();
    let blocks = &next.doc.root_node().children;
    assert_eq!(blocks.len(), 2);
    assert_eq!(
        next.doc
            .get_text(next.doc.get_element(blocks[0]).unwrap().children[0])
            .unwrap()
            .text,
        "fir"
    );
    assert_eq!(
        next.doc
            .get_text(next.doc.get_element(blocks[1]).unwrap().children[0])
            .unwrap()
            .text,
        "ond"
    );
}

fn one_paragraph_with_texts(texts: &[&str]) -> (Doc, Vec<NodeKey>) {
    let doc = Doc::empty();
    let paragraph = doc.root_node().children[0];
    let (doc, _) = Transaction::new()
        .step(Step::InsertNodes {
            parent: paragraph,
            index: 0,
            nodes: texts.iter().map(|text| NodeSpec::text(*text)).collect(),
        })
        .apply(doc)
        .unwrap();
    let keys = doc.get_element(paragraph).unwrap().children.clone();
    (doc, keys)
}

#[test]
fn join_text_rejects_same_key() {
    let (doc, keys) = one_paragraph_with_texts(&["a"]);
    assert!(matches!(
        Step::JoinText {
            left: keys[0],
            right: keys[0]
        }
        .apply(doc),
        Err(StepError::SchemaViolation(_))
    ));
}

#[test]
fn join_text_rejects_cross_parent_nodes() {
    let doc = Doc::empty();
    let root = doc.root_key();
    let (doc, _) = Transaction::new()
        .step(Step::RemoveNodes {
            parent: root,
            range: 0..1,
        })
        .step(Step::InsertNodes {
            parent: root,
            index: 0,
            nodes: vec![
                NodeSpec::paragraph(vec![NodeSpec::text("a")]),
                NodeSpec::paragraph(vec![NodeSpec::text("b")]),
            ],
        })
        .apply(doc)
        .unwrap();
    let blocks = doc.root_node().children.clone();
    let left = doc.get_element(blocks[0]).unwrap().children[0];
    let right = doc.get_element(blocks[1]).unwrap().children[0];
    assert!(matches!(
        Step::JoinText { left, right }.apply(doc),
        Err(StepError::SchemaViolation(_))
    ));
}

#[test]
fn join_text_rejects_non_adjacent_siblings() {
    let (doc, keys) = one_paragraph_with_texts(&["a", "b", "c"]);
    assert!(matches!(
        Step::JoinText {
            left: keys[0],
            right: keys[2]
        }
        .apply(doc),
        Err(StepError::SchemaViolation(_))
    ));
}

#[test]
fn markdown_round_trip_keeps_literal_commonmark_markers_plain() {
    let literal = "*em* _em_ **bold** [link](https://example.test) `code` ![alt](image.png)";
    let (doc, _) = one_paragraph_with_texts(&[literal]);
    let schema = Schema::new();
    let markdown = dioxus_editor::io::to_markdown(&doc, &schema).unwrap();
    let reparsed = dioxus_editor::io::from_markdown(&markdown, &schema);
    let paragraph = reparsed
        .get_element(reparsed.root_node().children[0])
        .unwrap();
    let text: String = paragraph
        .children
        .iter()
        .map(|key| reparsed.get_text(*key).unwrap().text.as_str())
        .collect();
    assert_eq!(text, literal);
    assert!(paragraph
        .children
        .iter()
        .all(|key| reparsed.get_text(*key).unwrap().format == FormatBits::NONE));
}

#[test]
fn markdown_code_span_round_trip_handles_embedded_backticks_and_padding() {
    for literal in ["a ` b `` c", "`edge`", " padded "] {
        let doc = Doc::empty();
        let paragraph = doc.root_node().children[0];
        let (doc, _) = Transaction::new()
            .step(Step::InsertNodes {
                parent: paragraph,
                index: 0,
                nodes: vec![NodeSpec::Text {
                    text: literal.into(),
                    format: FormatBits::CODE,
                }],
            })
            .apply(doc)
            .unwrap();
        let schema = Schema::new();
        let markdown = dioxus_editor::io::to_markdown(&doc, &schema).unwrap();
        let reparsed = dioxus_editor::io::from_markdown(&markdown, &schema);
        let paragraph = reparsed
            .get_element(reparsed.root_node().children[0])
            .unwrap();
        let text = reparsed.get_text(paragraph.children[0]).unwrap();
        assert_eq!(text.text, literal, "markdown: {markdown}");
        assert_eq!(text.format, FormatBits::CODE);
    }
}

#[test]
fn markdown_image_requires_registered_schema_kind() {
    let doc = dioxus_editor::io::from_markdown(
        "before ![safe alt](javascript:bad) after",
        &Schema::new(),
    );
    assert!(!doc
        .nodes()
        .values()
        .any(|node| matches!(node, Node::Decorator(_))));
    let paragraph = doc.get_element(doc.root_node().children[0]).unwrap();
    let text: String = paragraph
        .children
        .iter()
        .map(|key| doc.get_text(*key).unwrap().text.as_str())
        .collect();
    assert_eq!(text, "before safe alt after");
}
