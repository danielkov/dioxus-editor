//! Built-in commands.
//!
//! Commands are pure functions `&EditorState -> Option<Transaction>`. They
//! cover the editing primitives the view binds to (typing, deleting, enter,
//! mark toggling, decorator insertion) and are reusable from host code that
//! wants to drive the editor programmatically — e.g. the toolbar calls
//! [`toggle_bold`] directly.

use crate::attrs::Attrs;
use crate::format::FormatBits;
use crate::model::{Doc, Node, NodeKey};
use crate::selection::{Point, PointKind, Selection};
use crate::state::EditorState;
use crate::step::{NodeSpec, Step, Transaction};

/// Insert `text` at the current caret, deleting any selected range first.
/// Falls back to "end of doc" when there is no live selection — the editor
/// may have lost focus during an async upload or just mounted; typing
/// should never silently drop input.
pub fn insert_text(state: &EditorState, text: &str) -> Option<Transaction> {
    if text.is_empty() {
        return None;
    }
    let pending = state.pending_format;
    let sel = state.selection.clone();
    match sel {
        Selection::Range { anchor, focus } => {
            if anchor == focus {
                insert_text_at_caret(&state.doc, anchor, text, pending)
            } else {
                let (from, to) = order_points(&state.doc, anchor, focus);
                let mut tr = delete_range_transaction(&state.doc, from, to)?;
                // The delete may remove the text node `from` originally
                // pointed into (cross-block ranges replace both endpoint
                // blocks with one merged block). Build the insert
                // against a virtual post-delete state — using the delete
                // tr's resulting selection as the insertion caret —
                // instead of the still-unmodified source doc, otherwise
                // the insert step would reference a key that's about to
                // disappear and apply would fail silently.
                let virtual_state = state.apply(tr.clone()).ok()?;
                let caret_after_delete = match &virtual_state.selection {
                    Selection::Range { focus, .. } => *focus,
                    _ => return Some(tr),
                };
                if let Some(insert) =
                    insert_text_at_caret(&virtual_state.doc, caret_after_delete, text, pending)
                {
                    tr.steps.extend(insert.steps);
                    tr.selection = insert.selection;
                    if let Some(p) = insert.pending_format {
                        tr.pending_format = Some(p);
                    }
                }
                Some(tr)
            }
        }
        Selection::Node(_) | Selection::None => {
            let fallback = crate::plugins::last_leaf(&state.doc)?;
            let point = Point {
                key: fallback.0,
                offset: fallback.2,
                kind: fallback.1,
            };
            insert_text_at_caret(&state.doc, point, text, pending)
        }
    }
}

/// Format the next-typed text node should carry at `point`, given a
/// pending override. Pending wins when present; otherwise inherit only
/// the format of the text node the caret sits INSIDE. Element-anchored
/// carets (the post-markdown-shortcut state, the boundary between two
/// formatted runs, the start of an empty paragraph) default to NONE so
/// typing right after a `**bold**` closer doesn't keep extending the
/// bold span.
fn effective_format_at(doc: &Doc, point: Point, pending: Option<FormatBits>) -> FormatBits {
    if let Some(p) = pending {
        return p;
    }
    match point.kind {
        PointKind::Text => doc
            .get_text(point.key)
            .map(|t| t.format)
            .unwrap_or(FormatBits::NONE),
        PointKind::Element => FormatBits::NONE,
    }
}

fn active_format_at_caret(state: &EditorState) -> FormatBits {
    if let Some(p) = state.pending_format {
        return p;
    }
    let focus = match &state.selection {
        Selection::Range { focus, .. } => *focus,
        _ => return FormatBits::NONE,
    };
    effective_format_at(&state.doc, focus, None)
}

fn insert_text_at_caret(
    doc: &Doc,
    point: Point,
    text: &str,
    pending: Option<FormatBits>,
) -> Option<Transaction> {
    // Caret at root with Element kind would insert text as a root-level
    // sibling of the existing blocks — never what the user wants. Redirect
    // into the first child block (or fall back to last_leaf if root is
    // empty for some reason).
    if point.kind == PointKind::Element && point.key == doc.root {
        let kids = &doc.root_node().children;
        let target_block = kids.get(point.offset).or_else(|| kids.first()).copied();
        if let Some(block) = target_block {
            return insert_text_at_caret(doc, Point::element(block, 0), text, pending);
        }
    }
    let target = doc.get(point.key)?;
    let mut tr = Transaction::new();
    let target_format = effective_format_at(doc, point, pending);
    match (target, point.kind) {
        (Node::Text(t), PointKind::Text) => {
            // Pending overrides: split the surrounding text at the caret
            // and insert a fresh, differently-formatted text node so the
            // typed run carries the toggled mark instead of inheriting
            // its container's format. When the target format matches
            // (pending is None or equals t.format), extend in place.
            if target_format == t.format {
                let new_offset = t.text.chars().count().min(point.offset) + text.chars().count();
                tr.steps.push(Step::ReplaceText {
                    key: point.key,
                    from: point.offset,
                    to: point.offset,
                    text: text.to_string(),
                });
                tr.selection = Some(Selection::caret(Point::text(point.key, new_offset)));
                return Some(tr);
            }
            // pending differs from t.format. Split t into [pre, post]
            // at the caret, then insert a fresh text(target_format)
            // between them. Caret lands inside the fresh node.
            let (parent, idx) = doc.child_index(point.key)?;
            let t_len = t.text.chars().count();
            let pre: String = t.text.chars().take(point.offset).collect();
            let post: String = t.text.chars().skip(point.offset).collect();
            let pre_present = !pre.is_empty() && point.offset > 0;
            let post_present = !post.is_empty() && point.offset < t_len;
            let mut nodes: Vec<NodeSpec> = Vec::new();
            if pre_present {
                nodes.push(NodeSpec::Text {
                    text: pre,
                    format: t.format,
                });
            }
            nodes.push(NodeSpec::Text {
                text: text.to_string(),
                format: target_format,
            });
            if post_present {
                nodes.push(NodeSpec::Text {
                    text: post,
                    format: t.format,
                });
            }
            tr.steps.push(Step::RemoveNodes {
                parent,
                range: idx..idx + 1,
            });
            tr.steps.push(Step::InsertNodes {
                parent,
                index: idx,
                nodes,
            });
            let predicted = doc.peek_next_key();
            let fresh_key = predicted + if pre_present { 1 } else { 0 };
            tr.selection = Some(Selection::caret(Point::text(
                fresh_key,
                text.chars().count(),
            )));
            Some(tr)
        }
        (Node::Element(e), PointKind::Element) => {
            // Three cases when caret sits at an element-anchored offset:
            // 1. Previous sibling is a text node whose format matches the
            //    target — append.
            // 2. Next sibling is a text node whose format matches the
            //    target — prepend.
            // 3. No mergeable sibling — insert a fresh text node carrying
            //    the target format and aim the caret inside it.
            let prev_key = if point.offset > 0 {
                e.children.get(point.offset - 1).copied()
            } else {
                None
            };
            if let Some(prev) = prev_key {
                if let Some(prev_text) = doc.get_text(prev) {
                    if prev_text.format == target_format {
                        let new_offset = prev_text.text.chars().count() + text.chars().count();
                        tr.steps.push(Step::ReplaceText {
                            key: prev,
                            from: prev_text.text.chars().count(),
                            to: prev_text.text.chars().count(),
                            text: text.to_string(),
                        });
                        tr.selection = Some(Selection::caret(Point::text(prev, new_offset)));
                        return Some(tr);
                    }
                }
            }
            let next_key = e.children.get(point.offset).copied();
            if let Some(next) = next_key {
                if let Some(next_text) = doc.get_text(next) {
                    if next_text.format == target_format {
                        let new_offset = text.chars().count();
                        tr.steps.push(Step::ReplaceText {
                            key: next,
                            from: 0,
                            to: 0,
                            text: text.to_string(),
                        });
                        tr.selection = Some(Selection::caret(Point::text(next, new_offset)));
                        return Some(tr);
                    }
                }
            }
            let predicted = doc.peek_next_key();
            tr.steps.push(Step::InsertNodes {
                parent: e.key,
                index: point.offset,
                nodes: vec![NodeSpec::Text {
                    text: text.to_string(),
                    format: target_format,
                }],
            });
            tr.selection = Some(Selection::caret(Point::text(
                predicted,
                text.chars().count(),
            )));
            Some(tr)
        }
        _ => None,
    }
}

/// Delete the character/decorator immediately before the caret.
pub fn delete_backward(state: &EditorState) -> Option<Transaction> {
    let sel = state.selection.clone();
    if let Selection::Range { anchor, focus } = &sel {
        if anchor != focus {
            let (from, to) = order_points(&state.doc, *anchor, *focus);
            return delete_range_transaction(&state.doc, from, to);
        }
    }
    if let Selection::Node(key) = sel {
        let (parent, idx) = state.doc.child_index(key)?;
        return Some(
            Transaction::new()
                .step(Step::RemoveNodes {
                    parent,
                    range: idx..idx + 1,
                })
                .select(Selection::caret(Point::element(parent, idx))),
        );
    }
    let caret = match sel {
        Selection::Range { focus, .. } => focus,
        _ => return None,
    };

    // Editor-style demotion: Backspace at the very start of a heading or
    // blockquote demotes the block to a paragraph, preserving its
    // children. The user then presses Backspace again to join with the
    // previous block.
    if caret.offset == 0 {
        // Resolve the enclosing list_item / heading / blockquote when the
        // caret sits at the very start of its content. Both Text@0 (typical
        // typing position) and Element@0 (the empty-list-item case the
        // markdown shortcut produces, or the post-prune state after the
        // last char was deleted) need the same demotion/outdent semantics —
        // otherwise an empty bullet is unreachable by Backspace.
        let at_block_start = match caret.kind {
            PointKind::Text => state
                .doc
                .child_index(caret.key)
                .filter(|(_, idx)| *idx == 0)
                .map(|(parent, _)| parent),
            PointKind::Element => Some(caret.key),
        };
        if let Some(block_key) = at_block_start {
            if let Some(block_e) = state.doc.get_element(block_key) {
                // Demote heading / blockquote / code_block at
                // start of content collapse back to a plain paragraph,
                // preserving children. Code_block joins this set so an empty
                // code_block created via toolbar can be dismissed with one
                // Backspace — without it the user is stranded in the empty
                // block (and the `\n`-preserving split semantics make a
                // second Backspace inside it indistinguishable from the
                // first).
                if block_e.kind == "heading"
                    || block_e.kind == "blockquote"
                    || block_e.kind == "code_block"
                {
                    return set_block_kind(state, "paragraph", Attrs::new());
                }
                if block_e.kind == "list_item" {
                    if let Some((list_key, _)) = state.doc.child_index(block_key) {
                        if let Some(list_e) = state.doc.get_element(list_key) {
                            if (list_e.kind == "bullet_list" || list_e.kind == "ordered_list")
                                && state.doc.parent(list_key) == Some(state.doc.root)
                            {
                                // Empty item at any position: lift it out
                                // to a paragraph at the item's slot. Non-
                                // empty items fall through to the normal
                                // join-with-previous merge path (common
                                // behaviour: content items merge upward,
                                // empty items exit the list in place so
                                // the cursor doesn't hop back to the
                                // preceding bullet's end).
                                let item_is_empty = block_e.children.is_empty()
                                    || block_e.children.iter().all(|&k| {
                                        state
                                            .doc
                                            .get_text(k)
                                            .map(|t| t.text.is_empty())
                                            .unwrap_or(false)
                                    });
                                if item_is_empty
                                    || state
                                        .doc
                                        .child_index(block_key)
                                        .map(|(_, i)| i == 0)
                                        .unwrap_or(false)
                                {
                                    return list_outdent_item(state, list_key, block_key);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    match (state.doc.get(caret.key)?, caret.kind) {
        (Node::Text(t), PointKind::Text) => {
            if caret.offset == 0 {
                // Caret at start of text node: try joining with previous
                // sibling, or merge this paragraph with the one above.
                join_with_previous(&state.doc, caret.key)
            } else {
                let prev = caret.offset - 1;
                let len = t.text.chars().count();
                // If this delete would leave the text node empty, remove
                // it entirely — empty text fragments are structural
                // noise and the user-visible state should be a clean
                // `(paragraph)`.
                if len == 1 && prev == 0 {
                    let (parent, idx) = state.doc.child_index(caret.key)?;
                    return Some(
                        Transaction::new()
                            .step(Step::RemoveNodes {
                                parent,
                                range: idx..idx + 1,
                            })
                            .select(Selection::caret(Point::element(parent, idx))),
                    );
                }
                let tr = Transaction::new()
                    .step(Step::ReplaceText {
                        key: caret.key,
                        from: prev,
                        to: caret.offset,
                        text: String::new(),
                    })
                    .select(Selection::caret(Point::text(caret.key, prev)));
                Some(tr)
            }
        }
        (Node::Element(_), PointKind::Element) => {
            // Caret at element-level offset: descend into the previous
            // sibling. For text siblings, trim one char (or prune a
            // 1-char node); for decorators / nested elements, remove
            // the whole node — those are atomic.
            let elem = state.doc.get_element(caret.key)?;
            if caret.offset == 0 {
                return join_with_previous_element(&state.doc, caret.key);
            }
            let idx = caret.offset - 1;
            if idx >= elem.children.len() {
                return None;
            }
            let prev_key = elem.children[idx];
            if let Some(t) = state.doc.get_text(prev_key) {
                let len = t.text.chars().count();
                if len <= 1 {
                    return Some(
                        Transaction::new()
                            .step(Step::RemoveNodes {
                                parent: caret.key,
                                range: idx..idx + 1,
                            })
                            .select(Selection::caret(Point::element(caret.key, idx))),
                    );
                }
                return Some(
                    Transaction::new()
                        .step(Step::ReplaceText {
                            key: prev_key,
                            from: len - 1,
                            to: len,
                            text: String::new(),
                        })
                        .select(Selection::caret(Point::text(prev_key, len - 1))),
                );
            }
            let tr = Transaction::new()
                .step(Step::RemoveNodes {
                    parent: caret.key,
                    range: idx..idx + 1,
                })
                .select(Selection::caret(Point::element(caret.key, idx)));
            Some(tr)
        }
        _ => None,
    }
}

/// Delete the character/decorator immediately after the caret.
pub fn delete_forward(state: &EditorState) -> Option<Transaction> {
    let sel = state.selection.clone();
    if let Selection::Range { anchor, focus } = &sel {
        if anchor != focus {
            let (from, to) = order_points(&state.doc, *anchor, *focus);
            return delete_range_transaction(&state.doc, from, to);
        }
    }
    if let Selection::Node(key) = sel {
        let (parent, idx) = state.doc.child_index(key)?;
        return Some(
            Transaction::new()
                .step(Step::RemoveNodes {
                    parent,
                    range: idx..idx + 1,
                })
                .select(Selection::caret(Point::element(parent, idx))),
        );
    }
    let caret = match sel {
        Selection::Range { focus, .. } => focus,
        _ => return None,
    };
    match (state.doc.get(caret.key)?, caret.kind) {
        (Node::Text(t), PointKind::Text) => {
            let len = t.text.chars().count();
            if caret.offset >= len {
                // At end of text node. If this is the last child of its
                // block, Delete should join with the next block (the
                // forward mirror of Backspace-at-start-of-next).
                join_with_next(&state.doc, caret.key)
            } else if caret.offset + 1 == len && caret.offset == 0 {
                // The only-char-left case: removing it would leave an
                // empty text node. Prune the node instead.
                let (parent, idx) = state.doc.child_index(caret.key)?;
                Some(
                    Transaction::new()
                        .step(Step::RemoveNodes {
                            parent,
                            range: idx..idx + 1,
                        })
                        .select(Selection::caret(Point::element(parent, idx))),
                )
            } else {
                let tr = Transaction::new()
                    .step(Step::ReplaceText {
                        key: caret.key,
                        from: caret.offset,
                        to: caret.offset + 1,
                        text: String::new(),
                    })
                    .select(Selection::caret(caret));
                Some(tr)
            }
        }
        (Node::Element(e), PointKind::Element) => {
            if caret.offset >= e.children.len() {
                return None;
            }
            let next_key = e.children[caret.offset];
            if let Some(t) = state.doc.get_text(next_key) {
                let len = t.text.chars().count();
                if len <= 1 {
                    return Some(
                        Transaction::new()
                            .step(Step::RemoveNodes {
                                parent: caret.key,
                                range: caret.offset..caret.offset + 1,
                            })
                            .select(Selection::caret(caret)),
                    );
                }
                return Some(
                    Transaction::new()
                        .step(Step::ReplaceText {
                            key: next_key,
                            from: 0,
                            to: 1,
                            text: String::new(),
                        })
                        .select(Selection::caret(Point::text(next_key, 0))),
                );
            }
            let tr = Transaction::new()
                .step(Step::RemoveNodes {
                    parent: caret.key,
                    range: caret.offset..caret.offset + 1,
                })
                .select(Selection::caret(caret));
            Some(tr)
        }
        _ => None,
    }
}

/// Split the current block at the caret. Used for Enter / Shift+Enter
/// handling. If the selection is non-collapsed, the range is deleted first
/// and the split happens at the resulting caret.
///
/// Inside a `code_block`, Enter inserts a literal newline character into
/// the text node rather than splitting — a fenced markdown code block is
/// one logical block; multiple separate `code_block` elements would
/// serialize as multiple adjacent fences with empty paragraphs between,
/// which is the wrong shape.
pub fn split_block(state: &EditorState) -> Option<Transaction> {
    let (working, mut tr) = match &state.selection {
        Selection::Range { anchor, focus } if anchor == focus => {
            (state.clone(), Transaction::new())
        }
        Selection::Range { anchor, focus } => {
            let (from, to) = order_points(&state.doc, *anchor, *focus);
            let delete = delete_range_transaction(&state.doc, from, to)?;
            (state.apply(delete.clone()).ok()?, delete)
        }
        _ => return None,
    };
    let caret = match working.selection {
        Selection::Range { focus, .. } => focus,
        _ => return None,
    };

    // Structural decisions and predicted keys must use the post-delete
    // document: a cross-block deletion can remove both original endpoints.
    let (block_key, child_idx) = leaf_block_for_caret(&working.doc, caret)?;

    // Code block: insert a newline character at the caret, don't split.
    if working
        .doc
        .get_element(block_key)
        .is_some_and(|block| block.kind == "code_block")
    {
        if let Some(insert) = insert_text(&working, "\n") {
            tr.steps.extend(insert.steps);
            tr.selection = insert.selection;
        }
        return Some(tr);
    }

    let text_len = working
        .doc
        .get_text(caret.key)
        .map(|t| t.text.chars().count())
        .unwrap_or(0);
    let splits_text = caret.kind == PointKind::Text && caret.offset > 0 && caret.offset < text_len;
    let split_at = match caret.kind {
        PointKind::Text if splits_text => {
            tr.steps.push(Step::SplitText {
                key: caret.key,
                at: caret.offset,
            });
            child_idx + 1
        }
        PointKind::Text if caret.offset == 0 => child_idx,
        PointKind::Text => child_idx + 1,
        PointKind::Element => caret.offset,
    };

    let new_block_key = working.doc.peek_next_key() + usize::from(splits_text) as u64;
    tr.steps.push(Step::SplitElement {
        key: block_key,
        at: split_at,
    });

    if working
        .doc
        .get_element(block_key)
        .is_some_and(|block| block.kind == "heading")
    {
        tr.steps.push(Step::SetElementKind {
            key: new_block_key,
            kind: "paragraph".into(),
            attrs: Some(crate::attrs::Attrs::new()),
        });
    }

    tr.selection = Some(Selection::caret(Point::element(new_block_key, 0)));
    Some(tr)
}

/// Toggle a mark across the current selection.
///
/// - **Same text node**: rebuild as [pre, marked-mid, post] so only the
///   selected subrange takes the mark. Empty pieces are omitted.
/// - **Cross-node, same parent**: split the first/last touched text nodes
///   at the selection boundary, mark every fully-contained text node in
///   between, leave structure outside the range untouched.
/// - **Collapsed selection**: flip the bit in `pending_format` so the
///   next typed run carries the mark. The format
///   sticks across Shift+Enter and additional typing; toggle again to
///   clear it.
pub fn toggle_mark(state: &EditorState, mark: FormatBits) -> Option<Transaction> {
    let (from, to) = match &state.selection {
        Selection::Range { anchor, focus } if anchor != focus => {
            order_points(&state.doc, *anchor, *focus)
        }
        _ => {
            let active = active_format_at_caret(state);
            let next = FormatBits(active.0 ^ mark.0);
            return Some(Transaction::new().pending_format(next));
        }
    };

    if from.key == to.key && from.kind == PointKind::Text && to.kind == PointKind::Text {
        return toggle_mark_within_text_node(state, from.key, from.offset, to.offset, mark);
    }

    // Same-parent fast path keeps its coalescing behaviour (adjacent
    // same-format runs merge into one spec). Anything spanning two parents
    // — most commonly two list_items, two paragraphs, or paragraph +
    // blockquote under a Cmd+A — drops to the cross-block walker.
    if let (Some((p_from, _)), Some((p_to, _))) = (
        state.doc.child_index(from.key),
        state.doc.child_index(to.key),
    ) {
        if p_from == p_to {
            return toggle_mark_cross_node(state, from, to, mark);
        }
    }
    toggle_mark_cross_block(state, from, to, mark)
}

/// Mark toggle across endpoints that sit under different parents — the
/// `select-all on a 2-item list, then Cmd+B` case. Walks every text node
/// the range intersects in document order, then chains per-node toggles
/// through a virtual state so each subsequent step sees up-to-date key
/// predictions.
fn toggle_mark_cross_block(
    state: &EditorState,
    from: Point,
    to: Point,
    mark: FormatBits,
) -> Option<Transaction> {
    let touched = collect_touched_text_runs(&state.doc, from, to);
    if touched.is_empty() {
        return None;
    }
    // ProseMirror convention: if every touched run already has the mark,
    // toggling clears it everywhere; otherwise we normalize the mixed
    // range to "all on".
    let all_have = touched.iter().all(|(k, _, _)| {
        state
            .doc
            .get_text(*k)
            .map(|t| t.format.contains(mark))
            .unwrap_or(false)
    });
    let add = !all_have;

    let mut virtual_state = state.clone();
    let mut combined = Transaction::new();
    let mut first_anchor: Option<Point> = None;
    let mut last_focus: Option<Point> = None;
    for (key, lo, hi) in touched {
        let part = apply_mark_to_text_node(&virtual_state, key, lo, hi, mark, add)?;
        if let Some(Selection::Range { anchor, focus }) = &part.selection {
            if first_anchor.is_none() {
                first_anchor = Some(*anchor);
            }
            last_focus = Some(*focus);
        }
        virtual_state = virtual_state.apply(part.clone()).ok()?;
        combined.steps.extend(part.steps);
    }
    if let (Some(a), Some(f)) = (first_anchor, last_focus) {
        combined.selection = Some(Selection::Range {
            anchor: a,
            focus: f,
        });
    }
    Some(combined)
}

/// Force a mark add/remove on `[lo, hi)` of a single text node, splitting
/// the node into `[pre, mid, post]` as needed. Unlike
/// `toggle_mark_within_text_node`, the direction is dictated by `add`
/// (not flipped per-node) so a chain of these against a mixed-coverage
/// range normalizes consistently to "all on" or "all off".
fn apply_mark_to_text_node(
    state: &EditorState,
    key: NodeKey,
    lo: usize,
    hi: usize,
    mark: FormatBits,
    add: bool,
) -> Option<Transaction> {
    let t = state.doc.get_text(key)?;
    let n = t.text.chars().count();
    let lo = lo.min(n);
    let hi = hi.min(n);
    if lo >= hi {
        return None;
    }
    let cur = t.format;
    let new_format = if add {
        FormatBits(cur.0 | mark.0)
    } else {
        FormatBits(cur.0 & !mark.0)
    };
    // Already in target state — emit no steps but report the selection
    // bounds so the cross-block walker can still track anchor/focus.
    if new_format == cur {
        return Some(Transaction::new().select(Selection::Range {
            anchor: Point::text(key, lo),
            focus: Point::text(key, hi),
        }));
    }
    let (parent, idx) = state.doc.child_index(key)?;
    if lo == 0 && hi == n {
        return Some(
            Transaction::new()
                .step(Step::SetFormat {
                    key,
                    format: new_format,
                })
                .select(Selection::Range {
                    anchor: Point::text(key, 0),
                    focus: Point::text(key, n),
                }),
        );
    }
    let pre: String = t.text.chars().take(lo).collect();
    let mid: String = t.text.chars().skip(lo).take(hi - lo).collect();
    let post: String = t.text.chars().skip(hi).collect();
    let pre_present = !pre.is_empty();
    let mid_present = !mid.is_empty();
    let post_present = !post.is_empty();
    let mut nodes: Vec<NodeSpec> = Vec::new();
    if pre_present {
        nodes.push(NodeSpec::Text {
            text: pre,
            format: cur,
        });
    }
    if mid_present {
        nodes.push(NodeSpec::Text {
            text: mid,
            format: new_format,
        });
    }
    if post_present {
        nodes.push(NodeSpec::Text {
            text: post,
            format: cur,
        });
    }
    let predicted = state.doc.peek_next_key();
    let mid_key = predicted + if pre_present { 1 } else { 0 };
    let mid_len = hi - lo;
    Some(
        Transaction::new()
            .step(Step::RemoveNodes {
                parent,
                range: idx..idx + 1,
            })
            .step(Step::InsertNodes {
                parent,
                index: idx,
                nodes,
            })
            .select(Selection::Range {
                anchor: Point::text(mid_key, 0),
                focus: Point::text(mid_key, mid_len),
            }),
    )
}

/// Walk the doc depth-first and emit `(text_key, local_lo, local_hi)` for
/// every text node whose char span intersects `[from, to]` in document
/// order. Local offsets are clipped to each node's length; nodes entirely
/// inside the range get `(0, len)`. Decorators contribute one position
/// slot but are never returned — mark toggles don't apply to atoms.
fn collect_touched_text_runs(doc: &Doc, from: Point, to: Point) -> Vec<(NodeKey, usize, usize)> {
    let from_pos = document_position(doc, from);
    let to_pos = document_position(doc, to);
    if from_pos >= to_pos {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut counter = 0usize;
    walk_text_nodes(doc, doc.root, &mut counter, from_pos, to_pos, &mut out);
    out
}

fn walk_text_nodes(
    doc: &Doc,
    key: NodeKey,
    counter: &mut usize,
    from_pos: usize,
    to_pos: usize,
    out: &mut Vec<(NodeKey, usize, usize)>,
) {
    let Some(node) = doc.get(key) else {
        return;
    };
    match node {
        Node::Element(e) => {
            for &child in &e.children {
                *counter += 1;
                walk_text_nodes(doc, child, counter, from_pos, to_pos, out);
            }
        }
        Node::Text(t) => {
            let text_lo = *counter;
            let len = t.text.chars().count();
            let text_hi = text_lo + len;
            if text_lo < to_pos && from_pos < text_hi {
                let local_lo = from_pos.saturating_sub(text_lo);
                let local_hi = (to_pos - text_lo).min(len);
                if local_lo < local_hi {
                    out.push((t.key, local_lo, local_hi));
                }
            }
            *counter += len;
        }
        Node::Decorator(_) => {
            *counter += 1;
        }
    }
}

fn toggle_mark_within_text_node(
    state: &EditorState,
    key: NodeKey,
    from_char: usize,
    to_char: usize,
    mark: FormatBits,
) -> Option<Transaction> {
    let t = state.doc.get_text(key)?;
    let n = t.text.chars().count();
    let from_char = from_char.min(n);
    let to_char = to_char.min(n);
    if from_char >= to_char {
        return None;
    }
    let (parent, idx) = state.doc.child_index(key)?;

    let cur_format = t.format;
    let new_format = if cur_format.contains(mark) {
        FormatBits(cur_format.0 & !mark.0)
    } else {
        FormatBits(cur_format.0 | mark.0)
    };

    // Whole-node fast path: no structural change.
    if from_char == 0 && to_char == n {
        let tr = Transaction::new()
            .step(Step::SetFormat {
                key,
                format: new_format,
            })
            .select(Selection::Range {
                anchor: Point::text(key, from_char),
                focus: Point::text(key, to_char),
            });
        return Some(tr);
    }

    let pre: String = t.text.chars().take(from_char).collect();
    let mid: String = t
        .text
        .chars()
        .skip(from_char)
        .take(to_char - from_char)
        .collect();
    let post: String = t.text.chars().skip(to_char).collect();

    let pre_present = !pre.is_empty();
    let mid_present = !mid.is_empty();
    let post_present = !post.is_empty();

    let mut nodes = Vec::new();
    if pre_present {
        nodes.push(NodeSpec::Text {
            text: pre,
            format: cur_format,
        });
    }
    if mid_present {
        nodes.push(NodeSpec::Text {
            text: mid,
            format: new_format,
        });
    }
    if post_present {
        nodes.push(NodeSpec::Text {
            text: post,
            format: cur_format,
        });
    }

    let predicted = state.doc.peek_next_key();
    let mid_key = predicted + if pre_present { 1 } else { 0 };
    let mid_len = to_char - from_char;

    let tr = Transaction::new()
        .step(Step::RemoveNodes {
            parent,
            range: idx..idx + 1,
        })
        .step(Step::InsertNodes {
            parent,
            index: idx,
            nodes,
        })
        .select(Selection::Range {
            anchor: Point::text(mid_key, 0),
            focus: Point::text(mid_key, mid_len),
        });
    Some(tr)
}

fn toggle_mark_cross_node(
    state: &EditorState,
    from: Point,
    to: Point,
    mark: FormatBits,
) -> Option<Transaction> {
    // We support cross-node only when both endpoints sit in the same block
    // (the typical case). Mark every fully-covered text node and clip the
    // boundary nodes at the selection edges. Anything else (block-spanning
    // selection) falls through to the legacy "mark whole nodes touched"
    // behavior; this remains a safe over-approximation.
    let (parent_from, _from_idx) = state.doc.child_index(from.key)?;
    let (parent_to, _to_idx) = state.doc.child_index(to.key)?;
    if parent_from != parent_to {
        return None;
    }
    let parent = parent_from;
    let elem = state.doc.get_element(parent)?;

    // Order children by index.
    let from_idx = elem.children.iter().position(|&k| k == from.key)?;
    let to_idx = elem.children.iter().position(|&k| k == to.key)?;
    let (lo_idx, hi_idx) = if from_idx <= to_idx {
        (from_idx, to_idx)
    } else {
        (to_idx, from_idx)
    };

    // Decide direction by sampling formats of all touched text nodes.
    let touched: Vec<NodeKey> = elem.children[lo_idx..=hi_idx]
        .iter()
        .copied()
        .filter(|k| matches!(state.doc.get(*k), Some(Node::Text(_))))
        .collect();
    if touched.is_empty() {
        return None;
    }
    let all_have = touched.iter().all(|k| {
        state
            .doc
            .get_text(*k)
            .map(|t| t.format.contains(mark))
            .unwrap_or(false)
    });
    let toggle = |cur: FormatBits| -> FormatBits {
        if all_have {
            FormatBits(cur.0 & !mark.0)
        } else {
            FormatBits(cur.0 | mark.0)
        }
    };

    // Build the children-after-change list for `parent`, then commit via a
    // single RemoveNodes(lo..hi+1) + InsertNodes(lo, ...). Going through
    // wholesale rebuild keeps key-prediction simple — only one batch
    // allocates fresh keys, all of which we can index by offset from
    // `peek_next_key`.
    let mut new_specs: Vec<NodeSpec> = Vec::new();
    let mut new_mid_first: Option<usize> = None; // index of first "mid" spec
    let mut new_mid_last: Option<usize> = None;

    for (rel_idx, &child) in elem.children[lo_idx..=hi_idx].iter().enumerate() {
        let abs_idx = lo_idx + rel_idx;
        match state.doc.get(child)? {
            Node::Text(t) => {
                let n = t.text.chars().count();
                let local_from = if abs_idx == from_idx { from.offset } else { 0 };
                let local_to = if abs_idx == to_idx { to.offset } else { n };
                let (lo, hi) = if local_from <= local_to {
                    (local_from, local_to)
                } else {
                    (local_to, local_from)
                };
                let pre: String = t.text.chars().take(lo).collect();
                let mid: String = t.text.chars().skip(lo).take(hi - lo).collect();
                let post: String = t.text.chars().skip(hi).collect();

                if !pre.is_empty() {
                    new_specs.push(NodeSpec::Text {
                        text: pre,
                        format: t.format,
                    });
                }
                if !mid.is_empty() {
                    let new_format = toggle(t.format);
                    let i = new_specs.len();
                    new_specs.push(NodeSpec::Text {
                        text: mid,
                        format: new_format,
                    });
                    if new_mid_first.is_none() {
                        new_mid_first = Some(i);
                    }
                    new_mid_last = Some(i);
                }
                if !post.is_empty() {
                    new_specs.push(NodeSpec::Text {
                        text: post,
                        format: t.format,
                    });
                }
            }
            Node::Decorator(d) => {
                // Decorators stay structurally identical; toggle mark
                // doesn't apply to atomic embeds — pass through.
                new_specs.push(NodeSpec::Decorator {
                    kind: d.kind.clone(),
                    attrs: d.attrs.clone(),
                });
            }
            Node::Element(e) => {
                // Element children inside a block are unusual (links?).
                // Pass through unchanged.
                new_specs.push(NodeSpec::Element {
                    kind: e.kind.clone(),
                    attrs: e.attrs.clone(),
                    children: e
                        .children
                        .iter()
                        .filter_map(|&c| node_spec_from(&state.doc, c))
                        .collect(),
                });
            }
        }
    }

    // Compute the *block-level* char offset where the toggled range
    // starts and where it ends, BEFORE we coalesce. We need these to
    // re-derive the selection target after merging adjacent same-format
    // specs.
    let anchor_idx = new_mid_first?;
    let focus_idx = new_mid_last?;
    let chars_in_spec = |s: &NodeSpec| -> usize {
        match s {
            NodeSpec::Text { text, .. } => text.chars().count(),
            _ => 1,
        }
    };
    let mid_start_chars: usize = new_specs[..anchor_idx].iter().map(chars_in_spec).sum();
    let mid_end_chars: usize = new_specs[..=focus_idx].iter().map(chars_in_spec).sum();

    let coalesced = coalesce_text_specs(new_specs);
    let predicted = state.doc.peek_next_key();
    // Walk the coalesced specs to find the (spec_idx, offset_in_spec)
    // for the start and end char offsets.
    let locate = |target: usize| -> (u64, usize) {
        let mut acc = 0usize;
        for (i, spec) in coalesced.iter().enumerate() {
            let len = chars_in_spec(spec);
            if acc + len >= target {
                return (predicted + i as u64, target - acc);
            }
            acc += len;
        }
        (
            predicted + (coalesced.len().saturating_sub(1)) as u64,
            coalesced.last().map(chars_in_spec).unwrap_or(0),
        )
    };
    let (anchor_key, anchor_off) = locate(mid_start_chars);
    let (focus_key, focus_off) = locate(mid_end_chars);

    let tr = Transaction::new()
        .step(Step::RemoveNodes {
            parent,
            range: lo_idx..hi_idx + 1,
        })
        .step(Step::InsertNodes {
            parent,
            index: lo_idx,
            nodes: coalesced,
        })
        .select(Selection::Range {
            anchor: Point::text(anchor_key, anchor_off),
            focus: Point::text(focus_key, focus_off),
        });
    Some(tr)
}

/// Insert an atomic decorator at the caret.
///
/// Inline decorators (e.g. mention chips) slide between text fragments
/// inside the current block. Block decorators (e.g. image / file cards)
/// live at root level — the surrounding block is split at the caret and
/// the decorator is inserted between the halves so the rendered DOM
/// doesn't end up with a `<div>` nested inside a `<p>` (invalid HTML the
/// browser would re-parent on its own).
pub fn insert_decorator(
    state: &EditorState,
    kind: impl Into<String>,
    attrs: Attrs,
) -> Option<Transaction> {
    let caret = match &state.selection {
        Selection::Range { anchor, focus } if anchor == focus => *focus,
        _ => return None,
    };
    let kind = kind.into();
    let inline = state
        .schema
        .decorator(&kind)
        .map(|s| s.inline)
        .unwrap_or(true);

    if inline {
        return insert_inline_decorator(state, caret, kind, attrs);
    }
    insert_block_decorator(state, caret, kind, attrs)
}

fn insert_inline_decorator(
    state: &EditorState,
    caret: Point,
    kind: String,
    attrs: Attrs,
) -> Option<Transaction> {
    let (parent_key, child_idx) = block_for_caret(&state.doc, caret)?;
    let mut tr = Transaction::new();

    let insert_idx = match caret.kind {
        PointKind::Text => {
            let len = state
                .doc
                .get_text(caret.key)
                .map(|t| t.text.chars().count())
                .unwrap_or(0);
            if caret.offset > 0 && caret.offset < len {
                tr.steps.push(Step::SplitText {
                    key: caret.key,
                    at: caret.offset,
                });
                child_idx + 1
            } else if caret.offset == 0 {
                child_idx
            } else {
                child_idx + 1
            }
        }
        PointKind::Element => caret.offset,
    };

    tr.steps.push(Step::InsertNodes {
        parent: parent_key,
        index: insert_idx,
        nodes: vec![NodeSpec::decorator(kind, attrs)],
    });
    tr.selection = Some(Selection::caret(Point::element(parent_key, insert_idx + 1)));
    Some(tr)
}

fn insert_block_decorator(
    state: &EditorState,
    caret: Point,
    kind: String,
    attrs: Attrs,
) -> Option<Transaction> {
    // Find the top-level block containing the caret. Block decorators must
    // be root-level siblings of the surrounding block, not nested inside
    // it. For carets inside nested structures (lists, blockquotes) we
    // place the decorator next to the outer block rather than splitting
    // mid-structure — splitting a list to drop an image between two items
    // would surprise the user more than placing the image just after.
    let mut block = caret.key;
    if caret.kind == PointKind::Text {
        if let Some((parent, _)) = state.doc.child_index(caret.key) {
            block = parent;
        }
    }
    while let Some(parent) = state.doc.parent(block) {
        if parent == state.doc.root {
            break;
        }
        block = parent;
    }
    let (root, block_idx) = state.doc.child_index(block)?;
    if root != state.doc.root {
        return None;
    }
    let block_e = state.doc.get_element(block)?;
    let block_is_empty = block_e.children.is_empty()
        || (block_e.children.len() == 1
            && state
                .doc
                .get_text(block_e.children[0])
                .map(|t| t.text.is_empty())
                .unwrap_or(false));
    // Position the caret in a fresh trailing paragraph so the user can
    // keep typing immediately after the embed without an extra Enter.
    let predicted = state.doc.peek_next_key();
    let decorator_spec = NodeSpec::Decorator { kind, attrs };
    let trailing_para = NodeSpec::Element {
        kind: "paragraph".into(),
        attrs: Attrs::new(),
        children: Vec::new(),
    };
    let mut tr = Transaction::new();
    if block_is_empty {
        // Replace the empty paragraph with [decorator, paragraph].
        tr.steps.push(Step::RemoveNodes {
            parent: state.doc.root,
            range: block_idx..block_idx + 1,
        });
        tr.steps.push(Step::InsertNodes {
            parent: state.doc.root,
            index: block_idx,
            nodes: vec![decorator_spec, trailing_para],
        });
    } else {
        // Keep the existing block in place; insert decorator + paragraph
        // immediately after it.
        tr.steps.push(Step::InsertNodes {
            parent: state.doc.root,
            index: block_idx + 1,
            nodes: vec![decorator_spec, trailing_para],
        });
    }
    tr.selection = Some(Selection::caret(Point::element(predicted + 1, 0)));
    Some(tr)
}

/// Insert a link node (an inline `link` decorator) at the caret, replacing
/// any active selection. `text` is the visible label, `href` the
/// destination; the href is normalized so a bare `www.` host still
/// navigates. Used by the paste handler when the pasted text is a bare URL.
pub fn insert_link(state: &EditorState, href: &str, text: &str) -> Option<Transaction> {
    let attrs = link_attrs(href, text);
    match &state.selection {
        Selection::Range { anchor, focus } if anchor != focus => {
            let (from, to) = order_points(&state.doc, *anchor, *focus);
            link_replacing_range(state, from, to, attrs)
        }
        Selection::Range { focus, .. } => {
            insert_inline_decorator(state, *focus, "link".into(), attrs)
        }
        Selection::Node(_) | Selection::None => {
            let leaf = crate::plugins::last_leaf(&state.doc)?;
            let caret = Point {
                key: leaf.0,
                offset: leaf.2,
                kind: leaf.1,
            };
            insert_inline_decorator(state, caret, "link".into(), attrs)
        }
    }
}

/// Wrap the selected text in a link whose label is that text and whose
/// destination is `href`. Used when a URL is pasted over a non-empty
/// selection (Cmd+Shift+V style). A collapsed selection falls back to
/// inserting the URL itself as the label.
pub fn wrap_selection_as_link(state: &EditorState, href: &str) -> Option<Transaction> {
    let (from, to) = match &state.selection {
        Selection::Range { anchor, focus } if anchor != focus => {
            order_points(&state.doc, *anchor, *focus)
        }
        _ => return insert_link(state, href, href),
    };
    let label = selected_text(&state.doc, from, to);
    if label.is_empty() {
        return insert_link(state, href, href);
    }
    link_replacing_range(state, from, to, link_attrs(href, &label))
}

fn link_attrs(href: &str, text: &str) -> Attrs {
    Attrs::new()
        .with("href", crate::autolink::normalize_href(href))
        .with("text", text.to_string())
}

/// Delete `[from, to]` then drop a link decorator at the resulting caret.
/// Mirrors the range branch of [`insert_text`]: the insert is built against
/// the post-delete virtual state so its key predictions line up when the
/// combined transaction replays against the original doc.
fn link_replacing_range(
    state: &EditorState,
    from: Point,
    to: Point,
    attrs: Attrs,
) -> Option<Transaction> {
    let mut tr = delete_range_transaction(&state.doc, from, to)?;
    let virtual_state = state.apply(tr.clone()).ok()?;
    let caret = match &virtual_state.selection {
        Selection::Range { focus, .. } => *focus,
        _ => return Some(tr),
    };
    if let Some(insert) = insert_inline_decorator(&virtual_state, caret, "link".into(), attrs) {
        tr.steps.extend(insert.steps);
        tr.selection = insert.selection;
    }
    Some(tr)
}

/// Concatenate the text covered by `[from, to]`, skipping atomic nodes.
fn selected_text(doc: &Doc, from: Point, to: Point) -> String {
    let mut out = String::new();
    for (key, lo, hi) in collect_touched_text_runs(doc, from, to) {
        if let Some(t) = doc.get_text(key) {
            let seg: String = t.text.chars().skip(lo).take(hi - lo).collect();
            out.push_str(&seg);
        }
    }
    out
}

// -- helpers ---------------------------------------------------------------

pub(crate) fn order_points(doc: &Doc, a: Point, b: Point) -> (Point, Point) {
    if same_position(a, b) {
        return (a, b);
    }
    // Within the same node: offset comparison is sufficient.
    if a.key == b.key {
        return if a.offset <= b.offset { (a, b) } else { (b, a) };
    }
    // Cross-node: compute document order by walking the tree from root.
    let order_a = document_position(doc, a);
    let order_b = document_position(doc, b);
    if order_a <= order_b {
        (a, b)
    } else {
        (b, a)
    }
}

fn same_position(a: Point, b: Point) -> bool {
    a.key == b.key && a.offset == b.offset && a.kind == b.kind
}

/// Linear "document position" — a depth-first traversal index that places
/// every character and element-boundary in a total order. Used only for
/// comparing two `Point`s; not stable across edits.
fn document_position(doc: &Doc, point: Point) -> usize {
    let mut counter = 0usize;
    let mut target = None;
    walk_positions(doc, doc.root, &point, &mut counter, &mut target);
    target.unwrap_or(counter)
}

fn walk_positions(
    doc: &Doc,
    key: NodeKey,
    target: &Point,
    counter: &mut usize,
    out: &mut Option<usize>,
) {
    if let Some(node) = doc.get(key) {
        match node {
            Node::Element(e) => {
                if target.key == e.key && target.kind == PointKind::Element && target.offset == 0 {
                    *out = Some(*counter);
                    return;
                }
                for (idx, child) in e.children.iter().copied().enumerate() {
                    if target.key == e.key
                        && target.kind == PointKind::Element
                        && target.offset == idx
                    {
                        *out = Some(*counter);
                    }
                    *counter += 1;
                    walk_positions(doc, child, target, counter, out);
                }
                if target.key == e.key
                    && target.kind == PointKind::Element
                    && target.offset == e.children.len()
                {
                    *out = Some(*counter);
                }
            }
            Node::Text(t) => {
                if target.key == t.key && target.kind == PointKind::Text {
                    *out = Some(*counter + target.offset);
                }
                *counter += t.text.chars().count();
            }
            Node::Decorator(d) => {
                if target.key == d.key {
                    *out = Some(*counter);
                }
                *counter += 1;
            }
        }
    }
}

/// Innermost element containing the caret + the caret's index inside it.
/// For nested layouts (`list > list_item > text`) this returns the
/// `list_item`, not the `list`. Used wherever a command operates on the
/// container that "Enter" would naturally split.
fn leaf_block_for_caret(doc: &Doc, caret: Point) -> Option<(NodeKey, usize)> {
    match caret.kind {
        PointKind::Element => Some((caret.key, caret.offset)),
        PointKind::Text => doc.child_index(caret.key),
    }
}

/// Find the closest block-ish ancestor (the immediate child of `doc.root`)
/// containing the caret, and the caret's child index *inside that block*
/// (not the block's index in the doc root — that distinction is the
/// whole point of this helper).
fn block_for_caret(doc: &Doc, caret: Point) -> Option<(NodeKey, usize)> {
    if caret.kind == PointKind::Element {
        return Some((caret.key, caret.offset));
    }
    // Climb from the caret's text node; track the index inside the
    // immediate-child-of-block as we go.
    let mut cur = caret.key;
    while let Some(parent) = doc.parent(cur) {
        let (_, idx_in_block) = doc.child_index(cur)?;
        if parent == doc.root {
            return Some((parent, idx_in_block));
        }
        if doc.parent(parent) == Some(doc.root) {
            // `parent` is the block — `cur` is the block's direct child.
            // Caret index inside the block = idx of `cur` in `parent`.
            return Some((parent, idx_in_block));
        }
        cur = parent;
    }
    None
}

/// Build a transaction that deletes everything between two ordered points.
///
/// Handles three cases:
/// 1. Same text node — single `ReplaceText` step.
/// 2. Different text nodes inside the same block — trim the trailing tail
///    of `from`, drop the children strictly between, trim the leading head
///    of `to`. Caret lands at the boundary, in `from`'s text node.
/// 3. Cross-block — not yet supported; returns `None`. The view falls back
///    to the browser's default for now.
pub fn delete_range_transaction(doc: &Doc, from: Point, to: Point) -> Option<Transaction> {
    let base = base_delete_range_transaction(doc, from, to)?;
    Some(normalize_empty_non_paragraph_blocks(doc, base))
}

/// Original delete-range body — kept as a private helper so the public
/// entry point can layer a "no residual empty block kind" pass on top.
fn base_delete_range_transaction(doc: &Doc, from: Point, to: Point) -> Option<Transaction> {
    if same_position(from, to) {
        return None;
    }
    if from.key == to.key && from.kind == PointKind::Text && to.kind == PointKind::Text {
        let t = doc.get_text(from.key)?;
        let len = t.text.chars().count();
        // Whole-text-node deletion: prune the empty fragment instead of
        // leaving a stray `(text "")` child.
        if from.offset == 0 && to.offset == len {
            let (parent, idx) = doc.child_index(from.key)?;
            return Some(
                Transaction::new()
                    .step(Step::RemoveNodes {
                        parent,
                        range: idx..idx + 1,
                    })
                    .select(Selection::caret(Point::element(parent, idx))),
            );
        }
        return Some(
            Transaction::new()
                .step(Step::ReplaceText {
                    key: from.key,
                    from: from.offset,
                    to: to.offset,
                    text: String::new(),
                })
                .select(Selection::caret(Point::text(from.key, from.offset))),
        );
    }
    // Multi-node, same parent. Endpoints may be either text or element-
    // anchored — the typical case has two text endpoints.
    let (parent_from, from_idx) = doc.child_index(from.key)?;
    let (parent_to, to_idx) = doc.child_index(to.key)?;
    if parent_from != parent_to {
        return delete_range_cross_block(doc, from, to);
    }
    let parent = parent_from;
    let (lo_key, lo_idx, lo_off, hi_key, hi_idx, hi_off) = if from_idx <= to_idx {
        (from.key, from_idx, from.offset, to.key, to_idx, to.offset)
    } else {
        (to.key, to_idx, to.offset, from.key, from_idx, from.offset)
    };

    let mut tr = Transaction::new();

    // Figure out which boundary nodes would be left fully empty after
    // trimming — we'll drop them alongside the intermediate children in a
    // single RemoveNodes step so the parent doesn't accumulate zero-width
    // span ghosts.
    let lo_text = doc.get_text(lo_key);
    let hi_text = doc.get_text(hi_key);
    let lo_len = lo_text.map(|t| t.text.chars().count()).unwrap_or(0);
    let hi_len = hi_text.map(|t| t.text.chars().count()).unwrap_or(0);
    let lo_empty_after = lo_text.is_some() && lo_off == 0;
    let hi_empty_after = hi_text.is_some() && hi_off == hi_len;

    // 1. Trim the high-side node's leading chars (if it survives).
    if !hi_empty_after && hi_off > 0 {
        tr.steps.push(Step::ReplaceText {
            key: hi_key,
            from: 0,
            to: hi_off,
            text: String::new(),
        });
    }

    // 2. Trim the low-side node's trailing chars (if it survives).
    if !lo_empty_after && lo_off < lo_len {
        tr.steps.push(Step::ReplaceText {
            key: lo_key,
            from: lo_off,
            to: lo_len,
            text: String::new(),
        });
    }

    // 3. Single RemoveNodes covering all the children that should disappear:
    //    intermediate ones plus either boundary node if it'd be empty.
    let remove_lo_idx = if lo_empty_after { lo_idx } else { lo_idx + 1 };
    let remove_hi_idx = if hi_empty_after { hi_idx + 1 } else { hi_idx };
    if remove_lo_idx < remove_hi_idx {
        tr.steps.push(Step::RemoveNodes {
            parent,
            range: remove_lo_idx..remove_hi_idx,
        });
    }

    // Caret target: prefer the surviving low-side text node; otherwise the
    // high-side; otherwise the parent at the post-remove index.
    tr.selection = Some(if !lo_empty_after {
        Selection::caret(Point::text(lo_key, lo_off))
    } else if !hi_empty_after {
        // After remove, hi_key shifts left by the number of removed
        // children before it.
        Selection::caret(Point::text(hi_key, 0))
    } else {
        Selection::caret(Point::element(parent, remove_lo_idx))
    });
    Some(tr)
}

/// Cross-block delete: from is in block A, to is in block B (different
/// blocks). Merge A and B (and any blocks between them are dropped) into
/// a single block whose kind is taken from A. Boundary text nodes are
/// trimmed; everything in-between is discarded.
fn delete_range_cross_block(doc: &Doc, from: Point, to: Point) -> Option<Transaction> {
    let from_block = doc.parent(from.key)?;
    let to_block = doc.parent(to.key)?;
    if from_block == to_block {
        return None;
    }
    // Generalize for nested blocks (list items, blockquote inner paragraphs):
    // when either endpoint isn't a direct child of doc.root, the simple
    // "merge two top-level blocks" routine can't express the surviving
    // structure (a list with items above + paragraph in middle + items
    // below). Route those through the nested path. Endpoints that *are*
    // both direct children of doc.root take the original simple path
    // below — it coalesces same-format text at the join boundary, which
    // older tests pin down explicitly.
    let from_nested = doc.parent(from_block) != Some(doc.root);
    let to_nested = doc.parent(to_block) != Some(doc.root);
    if from_nested || to_nested {
        return delete_range_cross_block_nested(doc, from, to);
    }
    delete_range_cross_block_simple(doc, from, to)
}

/// After applying a delete, drop any top-level block that ended up with no
/// surviving children and isn't already a paragraph — that's the "no
/// residual block kind for fully-consumed nodes" rule. Works uniformly:
/// whether the user selected the entire textarea or just the last char
/// of a single blockquote, the surviving doc should never carry an empty
/// heading / blockquote / code_block / list. If the normalization drops
/// every top-level block, an empty paragraph is inserted as the new
/// floor so the editor never holds a doc with zero children.
fn normalize_empty_non_paragraph_blocks(doc: &Doc, base: Transaction) -> Transaction {
    let virtual_doc = match base.clone().apply(doc.clone()) {
        Ok((d, _)) => d,
        Err(_) => return base,
    };
    let root_children = virtual_doc.root_node().children.clone();
    let mut empty_idxs: Vec<usize> = Vec::new();
    for (i, &child) in root_children.iter().enumerate() {
        if let Some(e) = virtual_doc.get_element(child) {
            if e.children.is_empty() && e.kind != "paragraph" {
                empty_idxs.push(i);
            }
        }
    }
    if empty_idxs.is_empty() {
        return base;
    }
    // Group contiguous indices so we can issue a single Remove+Insert per
    // run — keeps the transaction tidy and avoids index-shift bookkeeping.
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut start = empty_idxs[0];
    let mut prev = start;
    for &i in &empty_idxs[1..] {
        if i == prev + 1 {
            prev = i;
        } else {
            runs.push((start, prev + 1));
            start = i;
            prev = i;
        }
    }
    runs.push((start, prev + 1));
    // Process runs from highest index downward so earlier-index deletions
    // don't shift later-index targets.
    let mut tr = base.clone();
    let predicted_base = virtual_doc.peek_next_key();
    // First inserted paragraph for caret retargeting — only meaningful
    // when the original selection's anchor was inside one of the dropped
    // blocks (covered below).
    let mut first_caret: Option<NodeKey> = None;
    let mut runs_for_caret = runs.clone();
    runs_for_caret.sort_by_key(|r| r.0);
    for (keys_in_order, (s, _e)) in runs_for_caret.iter().enumerate() {
        if *s == empty_idxs[0] {
            first_caret = Some(predicted_base + keys_in_order as u64);
        }
    }
    // Apply highest-index runs first so root.children indices don't drift.
    for &(s, e) in runs.iter().rev() {
        tr.steps.push(Step::RemoveNodes {
            parent: virtual_doc.root,
            range: s..e,
        });
        tr.steps.push(Step::InsertNodes {
            parent: virtual_doc.root,
            index: s,
            nodes: vec![NodeSpec::Element {
                kind: "paragraph".into(),
                attrs: Attrs::new(),
                children: Vec::new(),
            }],
        });
    }
    // Retarget selection if it landed in a dropped block. The base
    // selection is the post-base virtual state's selection; if its key
    // belongs to a now-empty dropped block, point it at the freshly-
    // inserted paragraph instead. Heuristic: if the base's selection
    // resolves to a key that's a descendant of (or equals) one of the
    // dropped blocks, retarget.
    if let Some(sel) = &base.selection {
        if let Some(point) = match sel {
            Selection::Range { focus, .. } => Some(*focus),
            _ => None,
        } {
            let in_dropped = root_children.iter().enumerate().any(|(i, &k)| {
                empty_idxs.contains(&i)
                    && (point.key == k || ancestor_chain_contains(&virtual_doc, point.key, k))
            });
            if in_dropped {
                if let Some(new_key) = first_caret {
                    tr.selection = Some(Selection::caret(Point::element(new_key, 0)));
                }
            }
        }
    }
    tr
}

fn ancestor_chain_contains(doc: &Doc, key: NodeKey, ancestor: NodeKey) -> bool {
    let mut cur = key;
    loop {
        if cur == ancestor {
            return true;
        }
        match doc.parent(cur) {
            Some(p) => cur = p,
            None => return false,
        }
    }
}

/// First ancestor of `key` whose parent is `doc.root`.
fn top_level_ancestor(doc: &Doc, key: NodeKey) -> Option<NodeKey> {
    let mut cur = key;
    loop {
        let parent = doc.parent(cur)?;
        if parent == doc.root {
            return Some(cur);
        }
        cur = parent;
    }
}

/// Specs for everything BEFORE `point` inside `root_block` (a direct child
/// of doc.root). Returns specs that should land at root level — for a
/// simple paragraph that's the trimmed paragraph itself; for a list it's
/// `[list(items_above), paragraph(text_in_item_up_to_caret)]` etc.
fn surviving_head_specs(doc: &Doc, root_block: NodeKey, point: Point) -> Vec<NodeSpec> {
    let block = match doc.get_element(root_block) {
        Some(b) => b,
        None => return Vec::new(),
    };
    let mut out: Vec<NodeSpec> = Vec::new();
    if block.kind == "bullet_list" || block.kind == "ordered_list" {
        // Find which list_item contains `point`.
        let item_key = block
            .children
            .iter()
            .copied()
            .find(|&item| doc_contains(doc, item, point.key));
        let Some(item_key) = item_key else {
            return out;
        };
        let item_pos = block
            .children
            .iter()
            .position(|&k| k == item_key)
            .unwrap_or(0);
        // Items strictly above the caret's item — keep verbatim inside a
        // shrunk list of the same kind.
        if item_pos > 0 {
            let items: Vec<NodeSpec> = block.children[..item_pos]
                .iter()
                .filter_map(|&k| node_spec_from(doc, k))
                .collect();
            out.push(NodeSpec::Element {
                kind: block.kind.clone(),
                attrs: block.attrs.clone(),
                children: items,
            });
        }
        // The trimmed item-paragraph for the caret's row, if it has
        // surviving content.
        let trimmed = trimmed_head_paragraph_for_item(doc, item_key, point);
        if let Some(p) = trimmed {
            out.push(p);
        }
        return out;
    }
    // Simple top-level block (paragraph / heading / blockquote / code_block).
    let trimmed = trimmed_head_paragraph_for_simple_block(doc, root_block, point);
    if let Some(p) = trimmed {
        out.push(p);
    }
    out
}

/// Specs for everything AFTER `point` inside `root_block`.
fn surviving_tail_specs(doc: &Doc, root_block: NodeKey, point: Point) -> Vec<NodeSpec> {
    let block = match doc.get_element(root_block) {
        Some(b) => b,
        None => return Vec::new(),
    };
    let mut out: Vec<NodeSpec> = Vec::new();
    if block.kind == "bullet_list" || block.kind == "ordered_list" {
        let item_key = block
            .children
            .iter()
            .copied()
            .find(|&item| doc_contains(doc, item, point.key));
        let Some(item_key) = item_key else {
            return out;
        };
        let item_pos = block
            .children
            .iter()
            .position(|&k| k == item_key)
            .unwrap_or(0);
        // Caret's item — keep tail as a paragraph.
        let trimmed = trimmed_tail_paragraph_for_item(doc, item_key, point);
        if let Some(p) = trimmed {
            out.push(p);
        }
        // Items strictly below — keep verbatim inside a shrunk list of the
        // same kind.
        if item_pos + 1 < block.children.len() {
            let items: Vec<NodeSpec> = block.children[item_pos + 1..]
                .iter()
                .filter_map(|&k| node_spec_from(doc, k))
                .collect();
            out.push(NodeSpec::Element {
                kind: block.kind.clone(),
                attrs: block.attrs.clone(),
                children: items,
            });
        }
        return out;
    }
    let trimmed = trimmed_tail_paragraph_for_simple_block(doc, root_block, point);
    if let Some(p) = trimmed {
        out.push(p);
    }
    out
}

fn doc_contains(doc: &Doc, ancestor: NodeKey, descendant: NodeKey) -> bool {
    let mut cur = descendant;
    loop {
        if cur == ancestor {
            return true;
        }
        match doc.parent(cur) {
            Some(p) => cur = p,
            None => return false,
        }
    }
}

fn trimmed_head_paragraph_for_item(doc: &Doc, item_key: NodeKey, point: Point) -> Option<NodeSpec> {
    let item = doc.get_element(item_key)?;
    let child_idx = item.children.iter().position(|&k| k == point.key)?;
    let mut kept: Vec<NodeSpec> = item.children[..child_idx]
        .iter()
        .filter_map(|&k| node_spec_from(doc, k))
        .collect();
    if let Some(t) = doc.get_text(point.key) {
        let head: String = t.text.chars().take(point.offset).collect();
        if !head.is_empty() {
            kept.push(NodeSpec::Text {
                text: head,
                format: t.format,
            });
        }
    }
    if kept.is_empty() {
        return None;
    }
    Some(NodeSpec::Element {
        kind: "paragraph".into(),
        attrs: Attrs::new(),
        children: kept,
    })
}

fn trimmed_tail_paragraph_for_item(doc: &Doc, item_key: NodeKey, point: Point) -> Option<NodeSpec> {
    let item = doc.get_element(item_key)?;
    let child_idx = item.children.iter().position(|&k| k == point.key)?;
    let mut kept: Vec<NodeSpec> = Vec::new();
    if let Some(t) = doc.get_text(point.key) {
        let tail: String = t.text.chars().skip(point.offset).collect();
        if !tail.is_empty() {
            kept.push(NodeSpec::Text {
                text: tail,
                format: t.format,
            });
        }
    }
    for &k in &item.children[child_idx + 1..] {
        if let Some(spec) = node_spec_from(doc, k) {
            kept.push(spec);
        }
    }
    if kept.is_empty() {
        return None;
    }
    Some(NodeSpec::Element {
        kind: "paragraph".into(),
        attrs: Attrs::new(),
        children: kept,
    })
}

fn trimmed_head_paragraph_for_simple_block(
    doc: &Doc,
    block_key: NodeKey,
    point: Point,
) -> Option<NodeSpec> {
    let block = doc.get_element(block_key)?;
    let child_idx = block.children.iter().position(|&k| k == point.key)?;
    let mut kept: Vec<NodeSpec> = block.children[..child_idx]
        .iter()
        .filter_map(|&k| node_spec_from(doc, k))
        .collect();
    if let Some(t) = doc.get_text(point.key) {
        let head: String = t.text.chars().take(point.offset).collect();
        if !head.is_empty() {
            kept.push(NodeSpec::Text {
                text: head,
                format: t.format,
            });
        }
    }
    if kept.is_empty() {
        return None;
    }
    Some(NodeSpec::Element {
        kind: block.kind.clone(),
        attrs: block.attrs.clone(),
        children: kept,
    })
}

fn trimmed_tail_paragraph_for_simple_block(
    doc: &Doc,
    block_key: NodeKey,
    point: Point,
) -> Option<NodeSpec> {
    let block = doc.get_element(block_key)?;
    let child_idx = block.children.iter().position(|&k| k == point.key)?;
    let mut kept: Vec<NodeSpec> = Vec::new();
    if let Some(t) = doc.get_text(point.key) {
        let tail: String = t.text.chars().skip(point.offset).collect();
        if !tail.is_empty() {
            kept.push(NodeSpec::Text {
                text: tail,
                format: t.format,
            });
        }
    }
    for &k in &block.children[child_idx + 1..] {
        if let Some(spec) = node_spec_from(doc, k) {
            kept.push(spec);
        }
    }
    if kept.is_empty() {
        return None;
    }
    Some(NodeSpec::Element {
        kind: block.kind.clone(),
        attrs: block.attrs.clone(),
        children: kept,
    })
}

/// Cross-block delete where at least one endpoint sits inside a nested
/// block (a list_item, blockquote-inner paragraph, …). Rebuilds the
/// affected top-level block(s) from the surviving head + tail. This is
/// what lets Cmd+A → Backspace on a list collapse to a clean paragraph
/// instead of no-op, and what makes Backspace across two items of the
/// same list cleanly merge them.
fn delete_range_cross_block_nested(doc: &Doc, from: Point, to: Point) -> Option<Transaction> {
    let (lo, hi) = order_points(doc, from, to);
    let lo_root_block = top_level_ancestor(doc, lo.key)?;
    let hi_root_block = top_level_ancestor(doc, hi.key)?;
    let (_, lo_root_idx) = doc.child_index(lo_root_block)?;
    let (_, hi_root_idx) = doc.child_index(hi_root_block)?;

    // Cross-item delete inside the SAME list: rebuild the list with
    // [items_above, merged_item(lo_head + hi_tail), items_below]. Without
    // this special case, the generic head+tail concat would emit two
    // paragraphs and lose the surrounding list_item kind — items that
    // are only partially intersected should keep their kind, per the
    // "no residual block kind ONLY for fully-consumed nodes" rule.
    if lo_root_block == hi_root_block {
        let block = doc.get_element(lo_root_block)?;
        if block.kind == "bullet_list" || block.kind == "ordered_list" {
            return delete_range_within_list(doc, lo_root_block, lo, hi, lo_root_idx);
        }
    }

    let head = surviving_head_specs(doc, lo_root_block, lo);
    let tail = surviving_tail_specs(doc, hi_root_block, hi);
    let mut new_specs: Vec<NodeSpec> = head;
    new_specs.extend(tail);
    if new_specs.is_empty() {
        new_specs.push(NodeSpec::Element {
            kind: "paragraph".into(),
            attrs: Attrs::new(),
            children: Vec::new(),
        });
    }
    let predicted = doc.peek_next_key();
    Some(
        Transaction::new()
            .step(Step::RemoveNodes {
                parent: doc.root,
                range: lo_root_idx..hi_root_idx + 1,
            })
            .step(Step::InsertNodes {
                parent: doc.root,
                index: lo_root_idx,
                nodes: new_specs,
            })
            .select(Selection::caret(Point::element(predicted, 0))),
    )
}

/// Cross-item delete inside a single list. Rebuilds the list as
/// `[items_above, merged_item, items_below]` where `merged_item` carries
/// the lo-item's surviving head plus the hi-item's surviving tail (text
/// from same-format adjacent runs is coalesced). If lo and hi turn out
/// to be in the same item, that item is just trimmed (delete inside a
/// list_item collapses to text deletion).
fn delete_range_within_list(
    doc: &Doc,
    list_key: NodeKey,
    lo: Point,
    hi: Point,
    list_root_idx: usize,
) -> Option<Transaction> {
    let list = doc.get_element(list_key)?;
    let lo_item_key = list
        .children
        .iter()
        .copied()
        .find(|&item| doc_contains(doc, item, lo.key))?;
    let hi_item_key = list
        .children
        .iter()
        .copied()
        .find(|&item| doc_contains(doc, item, hi.key))?;
    let lo_pos = list.children.iter().position(|&k| k == lo_item_key)?;
    let hi_pos = list.children.iter().position(|&k| k == hi_item_key)?;
    let lo_item = doc.get_element(lo_item_key)?;
    let lo_head_children = item_head_children(doc, lo_item, lo);
    let merged_children = if lo_item_key == hi_item_key {
        // Same item: trim head + tail within that item.
        let mut combined = lo_head_children;
        let tail = item_tail_children(doc, lo_item, hi);
        coalesce_into(&mut combined, tail);
        combined
    } else {
        let hi_item = doc.get_element(hi_item_key)?;
        let mut combined = lo_head_children;
        let hi_tail = item_tail_children(doc, hi_item, hi);
        coalesce_into(&mut combined, hi_tail);
        combined
    };

    let items_above_present = lo_pos > 0;
    let items_below_present = hi_pos + 1 < list.children.len();
    let merged_present = !merged_children.is_empty();

    // No surviving anything inside the list — the entire list was consumed.
    // Return a transaction that replaces the list with an empty paragraph;
    // the post-delete normaliser would otherwise leave a list with one
    // empty list_item child (which isn't considered "empty" at the top
    // level even though semantically nothing remains).
    if !items_above_present && !items_below_present && !merged_present {
        let predicted = doc.peek_next_key();
        return Some(
            Transaction::new()
                .step(Step::RemoveNodes {
                    parent: doc.root,
                    range: list_root_idx..list_root_idx + 1,
                })
                .step(Step::InsertNodes {
                    parent: doc.root,
                    index: list_root_idx,
                    nodes: vec![NodeSpec::Element {
                        kind: "paragraph".into(),
                        attrs: Attrs::new(),
                        children: Vec::new(),
                    }],
                })
                .select(Selection::caret(Point::element(predicted, 0))),
        );
    }

    let mut item_specs: Vec<NodeSpec> = Vec::new();
    for &k in &list.children[..lo_pos] {
        if let Some(spec) = node_spec_from(doc, k) {
            item_specs.push(spec);
        }
    }
    // Always emit the merged item (even empty) so the caret has a
    // list row to land on at the deletion site. Neighbouring items
    // keep the list alive structurally.
    item_specs.push(NodeSpec::Element {
        kind: "list_item".into(),
        attrs: Attrs::new(),
        children: merged_children,
    });
    for &k in &list.children[hi_pos + 1..] {
        if let Some(spec) = node_spec_from(doc, k) {
            item_specs.push(spec);
        }
    }

    let new_list_spec = NodeSpec::Element {
        kind: list.kind.clone(),
        attrs: list.attrs.clone(),
        children: item_specs,
    };
    let predicted = doc.peek_next_key();
    // predicted = new list key; +1 = first item; +2 = its first text/child
    // when present. We aim the caret at the start of the merged_item
    // (index = lo_pos within the new list).
    let merged_item_offset = lo_pos as u64 + 1;
    let merged_item_key = predicted + merged_item_offset;
    Some(
        Transaction::new()
            .step(Step::RemoveNodes {
                parent: doc.root,
                range: list_root_idx..list_root_idx + 1,
            })
            .step(Step::InsertNodes {
                parent: doc.root,
                index: list_root_idx,
                nodes: vec![new_list_spec],
            })
            .select(Selection::caret(Point::element(merged_item_key, 0))),
    )
}

/// Specs for the surviving content BEFORE `point` inside a list_item —
/// children up to point.key plus the text head of point.key when point
/// sits inside a text node.
fn item_head_children(doc: &Doc, item: &crate::model::ElementNode, point: Point) -> Vec<NodeSpec> {
    let child_idx = item
        .children
        .iter()
        .position(|&k| k == point.key)
        .unwrap_or(item.children.len());
    let mut out: Vec<NodeSpec> = item.children[..child_idx]
        .iter()
        .filter_map(|&k| node_spec_from(doc, k))
        .collect();
    if let Some(t) = doc.get_text(point.key) {
        let head: String = t.text.chars().take(point.offset).collect();
        if !head.is_empty() {
            out.push(NodeSpec::Text {
                text: head,
                format: t.format,
            });
        }
    }
    out
}

/// Specs for surviving content AFTER `point` inside a list_item.
fn item_tail_children(doc: &Doc, item: &crate::model::ElementNode, point: Point) -> Vec<NodeSpec> {
    let child_idx = item
        .children
        .iter()
        .position(|&k| k == point.key)
        .unwrap_or(item.children.len());
    let mut out: Vec<NodeSpec> = Vec::new();
    if let Some(t) = doc.get_text(point.key) {
        let tail: String = t.text.chars().skip(point.offset).collect();
        if !tail.is_empty() {
            out.push(NodeSpec::Text {
                text: tail,
                format: t.format,
            });
        }
    }
    for &k in &item.children[child_idx + 1..] {
        if let Some(spec) = node_spec_from(doc, k) {
            out.push(spec);
        }
    }
    out
}

/// Append `tail` specs onto `out`, coalescing adjacent same-format text
/// runs so a merged paragraph / list_item never carries two adjacent
/// `(text … :fmt X)(text … :fmt X)` pairs.
fn coalesce_into(out: &mut Vec<NodeSpec>, tail: Vec<NodeSpec>) {
    for spec in tail {
        if let NodeSpec::Text {
            text: incoming_text,
            format: incoming_format,
        } = &spec
        {
            if let Some(NodeSpec::Text {
                text: prev_text,
                format: prev_format,
            }) = out.last_mut()
            {
                if *prev_format == *incoming_format {
                    prev_text.push_str(incoming_text);
                    continue;
                }
            }
        }
        out.push(spec);
    }
}

/// Cross-block delete for the simple case where both endpoints are direct
/// children of top-level blocks (paragraph + paragraph, blockquote +
/// paragraph). Merges into a single block whose kind is taken from `from`'s
/// block; coalesces same-format text at the join boundary so tests pinning
/// the exact tree shape stay stable.
fn delete_range_cross_block_simple(doc: &Doc, from: Point, to: Point) -> Option<Transaction> {
    let from_block = doc.parent(from.key)?;
    let to_block = doc.parent(to.key)?;
    let (_, from_root_idx) = doc.child_index(from_block)?;
    let (_, to_root_idx) = doc.child_index(to_block)?;
    // Normalize order in doc.root's children.
    let (lo_block, lo_idx_root, lo_point, hi_block, hi_idx_root, hi_point) =
        if from_root_idx <= to_root_idx {
            (from_block, from_root_idx, from, to_block, to_root_idx, to)
        } else {
            (to_block, to_root_idx, to, from_block, from_root_idx, from)
        };

    let lo_para = doc.get_element(lo_block)?;
    let hi_para = doc.get_element(hi_block)?;
    let lo_child_idx = lo_para.children.iter().position(|&k| k == lo_point.key)?;
    let hi_child_idx = hi_para.children.iter().position(|&k| k == hi_point.key)?;

    let mut new_children: Vec<NodeSpec> = Vec::new();
    // Helper: append a text spec, merging with the previous one if it
    // exists with the same format. Without this, the cross-block delete
    // leaves the surviving heads/tails as two adjacent same-format text
    // nodes (e.g. "fir" + "ond" after deleting across "fir|st\nse|cond"),
    // which is structurally redundant and breaks tests that pin down the
    // exact tree.
    let push_text = |out: &mut Vec<NodeSpec>, text: String, format: FormatBits| {
        if text.is_empty() {
            return;
        }
        if let Some(NodeSpec::Text {
            text: prev_text,
            format: prev_format,
        }) = out.last_mut()
        {
            if *prev_format == format {
                prev_text.push_str(&text);
                return;
            }
        }
        out.push(NodeSpec::Text { text, format });
    };
    // Pre-from children of lo_block (siblings BEFORE the from-text node).
    for &k in &lo_para.children[..lo_child_idx] {
        if let Some(spec) = node_spec_from(doc, k) {
            new_children.push(spec);
        }
    }
    // from-text trimmed at the tail.
    if let Some(t) = doc.get_text(lo_point.key) {
        let kept: String = t.text.chars().take(lo_point.offset).collect();
        push_text(&mut new_children, kept, t.format);
    }
    // to-text trimmed at the head.
    if let Some(t) = doc.get_text(hi_point.key) {
        let kept: String = t.text.chars().skip(hi_point.offset).collect();
        push_text(&mut new_children, kept, t.format);
    }
    // Post-to children of hi_block.
    for &k in &hi_para.children[hi_child_idx + 1..] {
        if let Some(spec) = node_spec_from(doc, k) {
            new_children.push(spec);
        }
    }

    let merged_spec = NodeSpec::Element {
        kind: lo_para.kind.clone(),
        attrs: lo_para.attrs.clone(),
        children: new_children,
    };

    let predicted = doc.peek_next_key();
    let merged_block_key = predicted;
    // First text-or-leaf child of the new block: predicted + 1 (depth-
    // first allocation). Pre-from siblings contribute keys
    // predicted+1..predicted+lo_child_idx; the trimmed from-text lands at
    // predicted + 1 + lo_child_idx — unless `lo_point.offset == 0`, in
    // which case we omitted that node and the caret should land at the
    // start of whatever's next.
    let from_trimmed_present = doc
        .get_text(lo_point.key)
        .map(|t| {
            !t.text
                .chars()
                .take(lo_point.offset)
                .collect::<String>()
                .is_empty()
        })
        .unwrap_or(false);
    let caret_key = if from_trimmed_present {
        Some(predicted + 1 + lo_child_idx as u64)
    } else {
        None
    };
    let caret_sel = if let Some(k) = caret_key {
        Selection::caret(Point::text(k, lo_point.offset))
    } else {
        Selection::caret(Point::element(merged_block_key, lo_child_idx))
    };

    let tr = Transaction::new()
        .step(Step::RemoveNodes {
            parent: doc.root,
            range: lo_idx_root..hi_idx_root + 1,
        })
        .step(Step::InsertNodes {
            parent: doc.root,
            index: lo_idx_root,
            nodes: vec![merged_spec],
        })
        .select(caret_sel);
    Some(tr)
}

// -- modifier-aware delete commands ---------------------------------------

/// Delete from the caret back to the start of the current line. Targets
/// the *innermost* block (list_item, paragraph, heading…), so Cmd+Backspace
/// inside a list affects just the current item — not the whole list.
///
/// Inside a list_item the bullet is part of the line; the entire item is
/// removed (and the list collapses to an empty paragraph if this was the
/// only item) so the user gets a single keystroke to clear the line +
/// marker, matching the behavior of native macOS text fields where
/// Cmd+Backspace clears through the line break.
pub fn delete_to_block_start(state: &EditorState) -> Option<Transaction> {
    let caret = collapsed_caret(state)?;
    let (block, _) = leaf_block_for_caret(&state.doc, caret)?;
    let block_e = state.doc.get_element(block)?;
    if block_e.kind == "list_item" {
        return remove_list_item(state, block);
    }
    // Blockquote / code_block follow the list_item rule: the surrounding
    // block kind (the `>` marker, the code fence) is part of "the line", so
    // Cmd+Backspace removes the entire block and replaces it with an empty
    // paragraph at the same root position. Without this, Cmd+Backspace
    // inside a blockquote silently strands a now-empty `>` block.
    if block_e.kind == "blockquote" || block_e.kind == "code_block" {
        return remove_top_level_block_as_paragraph(state, block);
    }
    if block_e.children.is_empty() {
        return None;
    }
    let first = block_e.children[0];
    let start_point = match state.doc.get(first)? {
        Node::Text(_) => Point::text(first, 0),
        _ => Point::element(block, 0),
    };
    delete_range_transaction(&state.doc, start_point, caret)
}

/// Replace a top-level block (e.g. blockquote, code_block) with an empty
/// paragraph at the same root index. Returns `None` if `block` is not a
/// direct child of `doc.root` — nested blocks aren't in scope yet.
fn remove_top_level_block_as_paragraph(state: &EditorState, block: NodeKey) -> Option<Transaction> {
    let (root, idx) = state.doc.child_index(block)?;
    if root != state.doc.root {
        return None;
    }
    let predicted = state.doc.peek_next_key();
    Some(
        Transaction::new()
            .step(Step::RemoveNodes {
                parent: state.doc.root,
                range: idx..idx + 1,
            })
            .step(Step::InsertNodes {
                parent: state.doc.root,
                index: idx,
                nodes: vec![NodeSpec::Element {
                    kind: "paragraph".into(),
                    attrs: Attrs::new(),
                    children: Vec::new(),
                }],
            })
            .select(Selection::caret(Point::element(predicted, 0))),
    )
}

/// Delete from the caret forward to the end of the current line.
pub fn delete_to_block_end(state: &EditorState) -> Option<Transaction> {
    let caret = collapsed_caret(state)?;
    let (block, _) = leaf_block_for_caret(&state.doc, caret)?;
    let block_e = state.doc.get_element(block)?;
    if block_e.children.is_empty() {
        return None;
    }
    let last = *block_e.children.last()?;
    let end_point = match state.doc.get(last)? {
        Node::Text(t) => Point::text(last, t.text.chars().count()),
        _ => Point::element(block, block_e.children.len()),
    };
    delete_range_transaction(&state.doc, caret, end_point)
}

/// Remove an entire list_item, collapsing the surrounding list to a fresh
/// paragraph when the item was the only one. Caret lands at the end of the
/// previous item or, if the removed item was first, at the start of the
/// new first item (or in the freshly inserted paragraph).
fn remove_list_item(state: &EditorState, item_key: NodeKey) -> Option<Transaction> {
    let (list_key, item_idx) = state.doc.child_index(item_key)?;
    let list_e = state.doc.get_element(list_key)?;
    if list_e.kind != "bullet_list" && list_e.kind != "ordered_list" {
        return None;
    }
    let (root, list_idx) = state.doc.child_index(list_key)?;
    let predicted = state.doc.peek_next_key();
    let mut tr = Transaction::new();
    if list_e.children.len() == 1 {
        tr.steps.push(Step::RemoveNodes {
            parent: root,
            range: list_idx..list_idx + 1,
        });
        tr.steps.push(Step::InsertNodes {
            parent: root,
            index: list_idx,
            nodes: vec![NodeSpec::Element {
                kind: "paragraph".into(),
                attrs: Attrs::new(),
                children: Vec::new(),
            }],
        });
        tr.selection = Some(Selection::caret(Point::element(predicted, 0)));
    } else {
        tr.steps.push(Step::RemoveNodes {
            parent: list_key,
            range: item_idx..item_idx + 1,
        });
        if item_idx > 0 {
            let prev_key = list_e.children[item_idx - 1];
            let prev_e = state.doc.get_element(prev_key)?;
            tr.selection = Some(Selection::caret(Point::element(
                prev_key,
                prev_e.children.len(),
            )));
        } else {
            let new_first = list_e.children[1];
            tr.selection = Some(Selection::caret(Point::element(new_first, 0)));
        }
    }
    Some(tr)
}

/// Delete the word ending at the caret. Word ≈ run of non-whitespace
/// preceded by whitespace; we delete backwards over any whitespace
/// immediately before the caret, then over the preceding word.
pub fn delete_word_backward(state: &EditorState) -> Option<Transaction> {
    let caret = collapsed_caret(state)?;
    if caret.kind != PointKind::Text {
        return delete_backward(state);
    }
    let t = state.doc.get_text(caret.key)?;
    let chars: Vec<char> = t.text.chars().collect();
    let mut i = caret.offset.min(chars.len());
    while i > 0 && chars[i - 1].is_whitespace() {
        i -= 1;
    }
    while i > 0 && !chars[i - 1].is_whitespace() {
        i -= 1;
    }
    if i == caret.offset {
        // Already at word boundary — fall back to single-char delete (or
        // cross-node join via delete_backward).
        return delete_backward(state);
    }
    Some(
        Transaction::new()
            .step(Step::ReplaceText {
                key: caret.key,
                from: i,
                to: caret.offset,
                text: String::new(),
            })
            .select(Selection::caret(Point::text(caret.key, i))),
    )
}

/// Delete the word starting at the caret.
pub fn delete_word_forward(state: &EditorState) -> Option<Transaction> {
    let caret = collapsed_caret(state)?;
    if caret.kind != PointKind::Text {
        return delete_forward(state);
    }
    let t = state.doc.get_text(caret.key)?;
    let chars: Vec<char> = t.text.chars().collect();
    let n = chars.len();
    let mut i = caret.offset.min(n);
    while i < n && chars[i].is_whitespace() {
        i += 1;
    }
    while i < n && !chars[i].is_whitespace() {
        i += 1;
    }
    if i == caret.offset {
        return delete_forward(state);
    }
    Some(
        Transaction::new()
            .step(Step::ReplaceText {
                key: caret.key,
                from: caret.offset,
                to: i,
                text: String::new(),
            })
            .select(Selection::caret(Point::text(caret.key, caret.offset))),
    )
}

fn collapsed_caret(state: &EditorState) -> Option<Point> {
    match &state.selection {
        Selection::Range { anchor, focus } if anchor == focus => Some(*focus),
        _ => None,
    }
}

// -- block-level commands -------------------------------------------------

/// Change the kind of the block containing the caret. Preserves the
/// block's attributes and its children. Used for paragraph ↔ heading ↔
/// blockquote toggling.
pub fn set_block_kind(state: &EditorState, new_kind: &str, attrs: Attrs) -> Option<Transaction> {
    let caret = primary_caret(state).or_else(|| {
        crate::plugins::last_leaf(&state.doc).map(|(k, kind, off)| Point {
            key: k,
            offset: off,
            kind,
        })
    })?;
    let (block, _) = block_for_caret(&state.doc, caret)?;
    let cur = state.doc.get_element(block)?;
    if cur.kind == new_kind && cur.attrs == attrs {
        return None;
    }
    let (parent, idx) = state.doc.child_index(block)?;
    if parent != state.doc.root {
        return None;
    }
    let original_child_idx = match caret.kind {
        PointKind::Element if caret.key == block => caret.offset,
        _ => state
            .doc
            .child_index(caret.key)
            .map(|(_, i)| i)
            .unwrap_or(0),
    };
    // Strip empty text fragments while transferring children. They survive
    // a "type then delete every char" cycle as placeholder nodes; carrying
    // them into the new block kind leaves `(paragraph (text ""))` instead
    // of a clean `(paragraph)` after a block demotion (heading /
    // blockquote / code_block → paragraph). Non-empty text is preserved
    // verbatim.
    let children_specs: Vec<NodeSpec> = cur
        .children
        .iter()
        .filter_map(|&k| node_spec_from(&state.doc, k))
        .filter(|spec| !matches!(spec, NodeSpec::Text { text, .. } if text.is_empty()))
        .collect();
    let new_spec = NodeSpec::Element {
        kind: new_kind.to_string(),
        attrs,
        children: children_specs,
    };
    let predicted = state.doc.peek_next_key();
    let new_block_key = predicted;
    let tr = Transaction::new()
        .step(Step::RemoveNodes {
            parent: state.doc.root,
            range: idx..idx + 1,
        })
        .step(Step::InsertNodes {
            parent: state.doc.root,
            index: idx,
            nodes: vec![new_spec],
        })
        .select(Selection::caret(Point::element(
            new_block_key,
            original_child_idx,
        )));
    Some(tr)
}

/// Toggle the block kind: if the caret's block is already `target_kind`,
/// revert to `paragraph`; otherwise switch to `target_kind`.
pub fn toggle_block(state: &EditorState, target_kind: &str, attrs: Attrs) -> Option<Transaction> {
    let caret = primary_caret(state)?;
    let (block, _) = block_for_caret(&state.doc, caret)?;
    let cur = state.doc.get_element(block)?;
    if cur.kind == target_kind {
        set_block_kind(state, "paragraph", Attrs::new())
    } else {
        set_block_kind(state, target_kind, attrs)
    }
}

pub fn toggle_blockquote(state: &EditorState) -> Option<Transaction> {
    toggle_block(state, "blockquote", Attrs::new())
}

pub fn toggle_heading(state: &EditorState, level: i64) -> Option<Transaction> {
    let caret = primary_caret(state)?;
    let (block, _) = block_for_caret(&state.doc, caret)?;
    let cur = state.doc.get_element(block)?;
    if cur.kind == "heading" && cur.attrs.get_int("level") == Some(level) {
        return set_block_kind(state, "paragraph", Attrs::new());
    }
    set_block_kind(state, "heading", Attrs::new().with("level", level))
}

pub fn toggle_code_block(state: &EditorState) -> Option<Transaction> {
    toggle_block(state, "code_block", Attrs::new())
}

/// Wrap the current block in a list (bullet or ordered) of one item.
/// Conversely, if already inside a list of the requested kind, unwrap
/// JUST the caret's item — block toggles only ever affect the line the
/// cursor is on (or the lines under a range selection); the rest of the
/// list stays intact. Switching between bullet/ordered swaps the outer
/// list kind for the whole list (changing only one item's marker would
/// produce nested-list semantics the editor doesn't model).
pub fn toggle_list(state: &EditorState, list_kind: &str) -> Option<Transaction> {
    let caret = primary_caret(state).or_else(|| {
        // Fall back to end-of-doc so the toolbar button still works when
        // the editor lost focus during a previous transformation.
        crate::plugins::last_leaf(&state.doc).map(|(k, kind, off)| Point {
            key: k,
            offset: off,
            kind,
        })
    })?;
    let (block, _) = block_for_caret(&state.doc, caret)?;
    let cur = state.doc.get_element(block)?;
    let (_, idx) = state.doc.child_index(block)?;
    let predicted = state.doc.peek_next_key();

    // Caret inside a list of the requested kind → lift only the caret's
    // item out as a paragraph at its position. Single-item lists collapse
    // entirely; multi-item lists either shrink (edge item) or split
    // (middle item) around the freshly-promoted paragraph. The rest of
    // the items keep their bullet — the toggle is a per-line operation.
    if cur.kind == list_kind {
        let item_key = item_under_caret(state, block, caret);
        if let Some(item_key) = item_key {
            return list_outdent_item(state, block, item_key);
        }
        // No item resolvable (shouldn't happen with a live caret) — fall
        // back to unwrapping the whole list as a safety net.
        let mut para_specs: Vec<NodeSpec> = Vec::new();
        for &item_key in &cur.children {
            let item_children = state
                .doc
                .get_element(item_key)
                .map(|e| {
                    e.children
                        .iter()
                        .filter_map(|&k| node_spec_from(&state.doc, k))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            para_specs.push(NodeSpec::Element {
                kind: "paragraph".into(),
                attrs: Attrs::new(),
                children: item_children,
            });
        }
        if para_specs.is_empty() {
            para_specs.push(NodeSpec::Element {
                kind: "paragraph".into(),
                attrs: Attrs::new(),
                children: Vec::new(),
            });
        }
        let new_first_para_key = predicted;
        return Some(
            Transaction::new()
                .step(Step::RemoveNodes {
                    parent: state.doc.root,
                    range: idx..idx + 1,
                })
                .step(Step::InsertNodes {
                    parent: state.doc.root,
                    index: idx,
                    nodes: para_specs,
                })
                .select(Selection::caret(Point::element(new_first_para_key, 0))),
        );
    }

    // Already a list of the other kind → swap outer kind only, preserving
    // items (don't double-wrap).
    if cur.kind == "bullet_list" || cur.kind == "ordered_list" {
        let new_spec = NodeSpec::Element {
            kind: list_kind.into(),
            attrs: cur.attrs.clone(),
            children: cur
                .children
                .iter()
                .filter_map(|&k| node_spec_from(&state.doc, k))
                .collect(),
        };
        let new_list_key = predicted;
        let new_first_item = new_list_key + 1;
        return Some(
            Transaction::new()
                .step(Step::RemoveNodes {
                    parent: state.doc.root,
                    range: idx..idx + 1,
                })
                .step(Step::InsertNodes {
                    parent: state.doc.root,
                    index: idx,
                    nodes: vec![new_spec],
                })
                .select(Selection::caret(Point::element(new_first_item, 0))),
        );
    }

    // Otherwise wrap the current block's children inside a single
    // list_item inside a list of the requested kind.
    let item_spec = NodeSpec::Element {
        kind: "list_item".into(),
        attrs: Attrs::new(),
        children: cur
            .children
            .iter()
            .filter_map(|&k| node_spec_from(&state.doc, k))
            .collect(),
    };
    let list_spec = NodeSpec::Element {
        kind: list_kind.into(),
        attrs: Attrs::new(),
        children: vec![item_spec],
    };
    let new_list_key = predicted;
    let new_item_key = new_list_key + 1;
    Some(
        Transaction::new()
            .step(Step::RemoveNodes {
                parent: state.doc.root,
                range: idx..idx + 1,
            })
            .step(Step::InsertNodes {
                parent: state.doc.root,
                index: idx,
                nodes: vec![list_spec],
            })
            .select(Selection::caret(Point::element(new_item_key, 0))),
    )
}

pub fn toggle_bullet_list(state: &EditorState) -> Option<Transaction> {
    toggle_list(state, "bullet_list")
}
pub fn toggle_ordered_list(state: &EditorState) -> Option<Transaction> {
    toggle_list(state, "ordered_list")
}

fn primary_caret(state: &EditorState) -> Option<Point> {
    match &state.selection {
        Selection::Range { focus, .. } => Some(*focus),
        _ => None,
    }
}

/// Find which direct-child list_item of `list_key` contains the caret.
/// Walks up from the caret's anchor key until hitting a child of `list_key`
/// or returning `None` (caret isn't inside this list at all).
fn item_under_caret(state: &EditorState, list_key: NodeKey, caret: Point) -> Option<NodeKey> {
    let list = state.doc.get_element(list_key)?;
    // Element-anchored caret on the list itself: pick the offset-th item,
    // clamped — that's where a fresh focus lands after a kind swap.
    if caret.kind == PointKind::Element && caret.key == list_key {
        return list
            .children
            .get(caret.offset.min(list.children.len().saturating_sub(1)))
            .copied();
    }
    let mut cur = caret.key;
    loop {
        if list.children.contains(&cur) {
            return Some(cur);
        }
        cur = state.doc.parent(cur)?;
    }
}

fn join_with_previous(doc: &Doc, key: NodeKey) -> Option<Transaction> {
    let (parent, idx) = doc.child_index(key)?;
    if idx == 0 {
        return join_with_previous_element(doc, parent);
    }
    let pe = doc.get_element(parent)?;
    let prev_key = *pe.children.get(idx - 1)?;
    match doc.get(prev_key)? {
        Node::Decorator(_) => Some(
            Transaction::new()
                .step(Step::RemoveNodes {
                    parent,
                    range: idx - 1..idx,
                })
                .select(Selection::caret(Point::text(key, 0))),
        ),
        Node::Text(prev) => {
            let prev_len = prev.text.chars().count();
            let cur_format = doc.get_text(key)?.format;
            if prev.format == cur_format {
                Some(
                    Transaction::new()
                        .step(Step::JoinText {
                            left: prev_key,
                            right: key,
                        })
                        .select(Selection::caret(Point::text(prev_key, prev_len))),
                )
            } else {
                // Formats differ: just delete the last char of prev.
                if prev_len == 0 {
                    return None;
                }
                Some(
                    Transaction::new()
                        .step(Step::ReplaceText {
                            key: prev_key,
                            from: prev_len - 1,
                            to: prev_len,
                            text: String::new(),
                        })
                        .select(Selection::caret(Point::text(prev_key, prev_len - 1))),
                )
            }
        }
        _ => None,
    }
}

/// Lift a list item out of a top-level list, replacing it with a paragraph
/// AT THE ITEM'S ROOT POSITION — the cursor stays on that line instead of
/// hopping back to the previous block. Three shapes:
///
/// * sole item — list is removed entirely, paragraph slides in at the
///   list's root index.
/// * edge item (first or last) — list shrinks; paragraph slides in
///   immediately before or after it.
/// * middle item — list splits: items before stay in the original list,
///   a paragraph takes the lifted item's slot, items after move into a
///   fresh list of the same kind right below.
fn list_outdent_item(
    state: &EditorState,
    list_key: NodeKey,
    item_key: NodeKey,
) -> Option<Transaction> {
    let (root, list_idx) = state.doc.child_index(list_key)?;
    let list = state.doc.get_element(list_key)?;
    let item = state.doc.get_element(item_key)?;
    let item_pos = list.children.iter().position(|&k| k == item_key)?;
    let total = list.children.len();
    // Drop empty text specs so the outdented paragraph is a clean
    // `(paragraph)` rather than `(paragraph (text ""))`.
    let para_children: Vec<NodeSpec> = item
        .children
        .iter()
        .filter_map(|&k| node_spec_from(&state.doc, k))
        .filter(|spec| !matches!(spec, NodeSpec::Text { text, .. } if text.is_empty()))
        .collect();
    let para_spec = NodeSpec::Element {
        kind: "paragraph".into(),
        attrs: Attrs::new(),
        children: para_children,
    };
    let predicted = state.doc.peek_next_key();
    let mut tr = Transaction::new();
    if total == 1 {
        // Sole item: replace the whole list with a paragraph.
        tr.steps.push(Step::RemoveNodes {
            parent: root,
            range: list_idx..list_idx + 1,
        });
        tr.steps.push(Step::InsertNodes {
            parent: root,
            index: list_idx,
            nodes: vec![para_spec],
        });
        tr.selection = Some(Selection::caret(Point::element(predicted, 0)));
    } else if item_pos == 0 {
        tr.steps.push(Step::RemoveNodes {
            parent: list_key,
            range: 0..1,
        });
        tr.steps.push(Step::InsertNodes {
            parent: root,
            index: list_idx,
            nodes: vec![para_spec],
        });
        tr.selection = Some(Selection::caret(Point::element(predicted, 0)));
    } else if item_pos == total - 1 {
        tr.steps.push(Step::RemoveNodes {
            parent: list_key,
            range: item_pos..item_pos + 1,
        });
        tr.steps.push(Step::InsertNodes {
            parent: root,
            index: list_idx + 1,
            nodes: vec![para_spec],
        });
        tr.selection = Some(Selection::caret(Point::element(predicted, 0)));
    } else {
        // Middle item: split the list into two halves around the lifted
        // item. Trim the original list to items before `item_pos`, then
        // insert `[paragraph, second_list]` after it at root level.
        let items_below: Vec<NodeSpec> = list.children[item_pos + 1..]
            .iter()
            .filter_map(|&k| node_spec_from(&state.doc, k))
            .collect();
        let list_below_spec = NodeSpec::Element {
            kind: list.kind.clone(),
            attrs: list.attrs.clone(),
            children: items_below,
        };
        tr.steps.push(Step::RemoveNodes {
            parent: list_key,
            range: item_pos..total,
        });
        tr.steps.push(Step::InsertNodes {
            parent: root,
            index: list_idx + 1,
            nodes: vec![para_spec, list_below_spec],
        });
        // `predicted` is the new paragraph's key; the second list lives
        // at `predicted + para_subtree_size` but we only need the
        // paragraph for the caret target.
        tr.selection = Some(Selection::caret(Point::element(predicted, 0)));
    }
    Some(tr)
}

/// Forward mirror of `join_with_previous`. The caret sits at the end of
/// `key`'s text (its containing block's last child); pull the next block
/// in flush — coalescing same-format text nodes at the join boundary.
fn join_with_next(doc: &Doc, key: NodeKey) -> Option<Transaction> {
    let (block_key, idx_in_block) = doc.child_index(key)?;
    let block = doc.get_element(block_key)?;
    // Only fire when the caret sits at the END of the block — otherwise
    // the next sibling within the same block should be the target, and
    // that's the normal in-block delete path.
    if idx_in_block + 1 != block.children.len() {
        return None;
    }
    let (root, block_idx) = doc.child_index(block_key)?;
    let parent_e = doc.get_element(root)?;
    let next_block_key = *parent_e.children.get(block_idx + 1)?;
    // Delete at end of a block whose next sibling is a block decorator
    // (image / file card / …) removes the decorator and keeps the caret
    // at end of the current block — there's no text to merge across.
    if matches!(doc.get(next_block_key), Some(Node::Decorator(_))) {
        let len = doc
            .get_text(key)
            .map(|t| t.text.chars().count())
            .unwrap_or(0);
        return Some(
            Transaction::new()
                .step(Step::RemoveNodes {
                    parent: root,
                    range: block_idx + 1..block_idx + 2,
                })
                .select(Selection::caret(Point::text(key, len))),
        );
    }
    let next_block = doc.get_element(next_block_key)?;
    let mut tr = Transaction::new();

    // Coalesce: if the boundary nodes are same-format text, splice the
    // first child of next_block into this block's last text node via
    // ReplaceText. Skip that child when re-inserting.
    let prev_text = doc.get_text(key)?;
    let prev_len = prev_text.text.chars().count();
    let prev_format = prev_text.format;
    let next_first_key = next_block.children.first().copied();
    let mut skip_first_next = false;
    let mut caret_text_target: Option<(NodeKey, usize)> = None;
    if let Some(first_key) = next_first_key {
        if let Some(next_text) = doc.get_text(first_key) {
            if next_text.format == prev_format {
                tr.steps.push(Step::ReplaceText {
                    key,
                    from: prev_len,
                    to: prev_len,
                    text: next_text.text.clone(),
                });
                caret_text_target = Some((key, prev_len));
                skip_first_next = true;
            }
        }
    }

    let block_child_count_after_merge = block.children.len();
    let remaining: Vec<NodeSpec> = next_block
        .children
        .iter()
        .enumerate()
        .filter_map(|(i, &k)| {
            if skip_first_next && i == 0 {
                None
            } else {
                node_spec_from(doc, k)
            }
        })
        .collect();
    if !remaining.is_empty() {
        tr.steps.push(Step::InsertNodes {
            parent: block_key,
            index: block_child_count_after_merge,
            nodes: remaining,
        });
    }
    tr.steps.push(Step::RemoveNodes {
        parent: root,
        range: block_idx + 1..block_idx + 2,
    });
    tr.selection = Some(if let Some((k, off)) = caret_text_target {
        Selection::caret(Point::text(k, off))
    } else {
        Selection::caret(Point::element(block_key, block_child_count_after_merge))
    });
    Some(tr)
}

fn join_with_previous_element(doc: &Doc, block_key: NodeKey) -> Option<Transaction> {
    let (parent, idx) = doc.child_index(block_key)?;
    if idx == 0 {
        return None;
    }
    let pe = doc.get_element(parent)?;
    let prev_key = *pe.children.get(idx - 1)?;
    // Backspace at the start of a block whose previous sibling is a block
    // decorator (image, file card, …) removes the decorator and parks the
    // caret at the start of the current block — there's no text to merge.
    if matches!(doc.get(prev_key), Some(Node::Decorator(_))) {
        let cur_e = doc.get_element(block_key)?;
        let caret = match cur_e.children.first().copied() {
            Some(first) => match doc.get(first)? {
                Node::Text(_) => Point::text(first, 0),
                _ => Point::element(block_key, 0),
            },
            None => Point::element(block_key, 0),
        };
        return Some(
            Transaction::new()
                .step(Step::RemoveNodes {
                    parent,
                    range: idx - 1..idx,
                })
                .select(Selection::caret(caret)),
        );
    }
    let _prev = doc.get_element(prev_key)?;
    let cur = doc.get_element(block_key)?;
    let cur_children = cur.children.clone();
    let mut tr = Transaction::new();

    // If the boundary nodes are same-format text, splice them: extend the
    // previous block's last text node with the current block's first text
    // node's content, and skip the first child when re-inserting. Without
    // this, joining "one" + "two" leaves two adjacent same-format text
    // nodes — structurally redundant and noisy in the model.
    let prev_kids = doc
        .get_element(prev_key)
        .map(|e| e.children.clone())
        .unwrap_or_default();
    let prev_last_key = prev_kids.last().copied();
    let cur_first_key = cur_children.first().copied();
    let mut skip_first_cur = false;
    let mut caret_text_target: Option<(NodeKey, usize)> = None;
    if let (Some(p), Some(c)) = (prev_last_key, cur_first_key) {
        if let (Some(p_text), Some(c_text)) = (doc.get_text(p), doc.get_text(c)) {
            if p_text.format == c_text.format {
                let p_len = p_text.text.chars().count();
                let merge_text = c_text.text.clone();
                tr.steps.push(Step::ReplaceText {
                    key: p,
                    from: p_len,
                    to: p_len,
                    text: merge_text,
                });
                caret_text_target = Some((p, p_len));
                skip_first_cur = true;
            }
        }
    }

    // Re-insert the rest of cur's children at the end of prev.
    let prev_child_count_after_merge = prev_kids.len();
    let remaining: Vec<NodeSpec> = cur_children
        .iter()
        .enumerate()
        .filter_map(|(i, &k)| {
            if skip_first_cur && i == 0 {
                None
            } else {
                node_spec_from(doc, k)
            }
        })
        .collect();
    if !remaining.is_empty() {
        tr.steps.push(Step::InsertNodes {
            parent: prev_key,
            index: prev_child_count_after_merge,
            nodes: remaining,
        });
    }
    // Remove the now-empty current block from its parent.
    tr.steps.push(Step::RemoveNodes {
        parent,
        range: idx..idx + 1,
    });
    // Caret lands inside the merged text node (if we coalesced) or at the
    // join point as a parent-anchored element offset.
    tr.selection = Some(if let Some((key, off)) = caret_text_target {
        Selection::caret(Point::text(key, off))
    } else {
        Selection::caret(Point::element(prev_key, prev_child_count_after_merge))
    });
    Some(tr)
}

/// Walk a list of `NodeSpec`s, merging any pair of adjacent `Text` specs
/// that share the same format bitfield. Same-format adjacency is a
/// structural inefficiency — mark toggles, range deletes, and block
/// merges all create transient runs of identical-format text that the
/// downstream renderer treats as separate spans for no benefit.
fn coalesce_text_specs(specs: Vec<NodeSpec>) -> Vec<NodeSpec> {
    let mut out: Vec<NodeSpec> = Vec::with_capacity(specs.len());
    for spec in specs {
        if let NodeSpec::Text { text, format } = &spec {
            if let Some(NodeSpec::Text {
                text: prev_text,
                format: prev_format,
            }) = out.last_mut()
            {
                if *prev_format == *format {
                    prev_text.push_str(text);
                    continue;
                }
            }
        }
        out.push(spec);
    }
    out
}

fn node_spec_from(doc: &Doc, key: NodeKey) -> Option<NodeSpec> {
    match doc.get(key)? {
        Node::Element(e) => Some(NodeSpec::Element {
            kind: e.kind.clone(),
            attrs: e.attrs.clone(),
            children: e
                .children
                .iter()
                .filter_map(|&c| node_spec_from(doc, c))
                .collect(),
        }),
        Node::Text(t) => Some(NodeSpec::Text {
            text: t.text.clone(),
            format: t.format,
        }),
        Node::Decorator(d) => Some(NodeSpec::Decorator {
            kind: d.kind.clone(),
            attrs: d.attrs.clone(),
        }),
    }
}

// Convenience commands for the toolbar / keymap.

pub fn toggle_bold(state: &EditorState) -> Option<Transaction> {
    toggle_mark(state, FormatBits::BOLD)
}
pub fn toggle_italic(state: &EditorState) -> Option<Transaction> {
    toggle_mark(state, FormatBits::ITALIC)
}
pub fn toggle_strike(state: &EditorState) -> Option<Transaction> {
    toggle_mark(state, FormatBits::STRIKE)
}
pub fn toggle_code(state: &EditorState) -> Option<Transaction> {
    toggle_mark(state, FormatBits::CODE)
}

/// Insert a fresh table at the caret's enclosing top-level block. The new
/// table has one header row and `rows - 1` body rows, each `cols` cells
/// wide. Caret lands in the first header cell. Empty caret blocks are
/// replaced; non-empty blocks gain the table as a sibling immediately
/// after, plus a trailing empty paragraph so the user can keep typing
/// past the table without an extra Enter.
pub fn insert_table(state: &EditorState, rows: usize, cols: usize) -> Option<Transaction> {
    let rows = rows.max(1);
    let cols = cols.max(1);
    let caret = primary_caret(state).or_else(|| {
        crate::plugins::last_leaf(&state.doc).map(|(k, kind, off)| Point {
            key: k,
            offset: off,
            kind,
        })
    })?;
    // Find the top-level block under doc.root that contains the caret.
    let mut block = caret.key;
    if caret.kind == PointKind::Text {
        if let Some((parent, _)) = state.doc.child_index(caret.key) {
            block = parent;
        }
    }
    while let Some(parent) = state.doc.parent(block) {
        if parent == state.doc.root {
            break;
        }
        block = parent;
    }
    let (root, block_idx) = state.doc.child_index(block)?;
    if root != state.doc.root {
        return None;
    }
    let block_e = state.doc.get_element(block)?;
    let block_is_empty = block_e.children.is_empty()
        || (block_e.children.len() == 1
            && state
                .doc
                .get_text(block_e.children[0])
                .map(|t| t.text.is_empty())
                .unwrap_or(false));

    let make_cell = || NodeSpec::Element {
        kind: "table_cell".into(),
        attrs: Attrs::new(),
        children: Vec::new(),
    };
    let make_row = |is_header: bool| NodeSpec::Element {
        kind: "table_row".into(),
        attrs: if is_header {
            Attrs::new().with("header", true)
        } else {
            Attrs::new()
        },
        children: (0..cols).map(|_| make_cell()).collect(),
    };
    let mut row_specs: Vec<NodeSpec> = Vec::with_capacity(rows);
    row_specs.push(make_row(true));
    for _ in 1..rows {
        row_specs.push(make_row(false));
    }
    let table_spec = NodeSpec::Element {
        kind: "table".into(),
        attrs: Attrs::new(),
        children: row_specs,
    };
    let trailing_para = NodeSpec::Element {
        kind: "paragraph".into(),
        attrs: Attrs::new(),
        children: Vec::new(),
    };

    let predicted = state.doc.peek_next_key();
    // materialize_into walks depth-first: table = predicted, row0 =
    // predicted + 1, then the row's `cols` cells starting at
    // predicted + 2. Land the caret in the first header cell.
    let first_cell_key = predicted + 2;

    let mut tr = Transaction::new();
    if block_is_empty {
        tr.steps.push(Step::RemoveNodes {
            parent: state.doc.root,
            range: block_idx..block_idx + 1,
        });
        tr.steps.push(Step::InsertNodes {
            parent: state.doc.root,
            index: block_idx,
            nodes: vec![table_spec, trailing_para],
        });
    } else {
        tr.steps.push(Step::InsertNodes {
            parent: state.doc.root,
            index: block_idx + 1,
            nodes: vec![table_spec, trailing_para],
        });
    }
    tr.selection = Some(Selection::caret(Point::element(first_cell_key, 0)));
    Some(tr)
}

/// Resolved position of the caret inside a table, surfaced for the
/// row/column commands. `cols` is the table's effective column count (max
/// over all rows).
#[derive(Clone, Copy, Debug)]
pub struct TableContext {
    pub table_key: NodeKey,
    /// Index of the table itself among `doc.root`'s children.
    pub table_idx_in_root: usize,
    pub row_key: NodeKey,
    pub row_idx: usize,
    pub cell_key: NodeKey,
    pub col_idx: usize,
    pub rows: usize,
    pub cols: usize,
}

/// Resolve the caret to its enclosing table cell. Returns `None` when the
/// caret isn't inside a `table_cell` node at all, or when the tree shape
/// doesn't match `table > row > cell`.
pub fn table_context(state: &EditorState) -> Option<TableContext> {
    let caret = primary_caret(state)?;
    table_context_for_point(&state.doc, caret)
}

fn table_context_for_point(doc: &Doc, caret: Point) -> Option<TableContext> {
    // Climb to a table_cell ancestor.
    let mut cur = caret.key;
    let cell_key = loop {
        let kind = doc.get(cur).map(Node::kind).unwrap_or("");
        if kind == "table_cell" {
            break cur;
        }
        cur = doc.parent(cur)?;
        if cur == doc.root {
            return None;
        }
    };
    let (row_key, col_idx) = doc.child_index(cell_key)?;
    let row = doc.get_element(row_key)?;
    if row.kind != "table_row" {
        return None;
    }
    let (table_key, row_idx) = doc.child_index(row_key)?;
    let table = doc.get_element(table_key)?;
    if table.kind != "table" {
        return None;
    }
    let (root, table_idx_in_root) = doc.child_index(table_key)?;
    if root != doc.root {
        return None;
    }
    let cols = table
        .children
        .iter()
        .filter_map(|&k| doc.get_element(k))
        .map(|r| r.children.len())
        .max()
        .unwrap_or(0);
    let rows = table.children.len();
    Some(TableContext {
        table_key,
        table_idx_in_root,
        row_key,
        row_idx,
        cell_key,
        col_idx,
        rows,
        cols,
    })
}

/// Split a comma-separated `align` attr into a Vec, padding with `"none"`
/// up to `len`. Joined back via [`join_align`].
fn split_align(attrs: &Attrs, len: usize) -> Vec<String> {
    let raw = attrs.get_str("align").unwrap_or("");
    let mut v: Vec<String> = if raw.is_empty() {
        Vec::new()
    } else {
        raw.split(',').map(|s| s.to_string()).collect()
    };
    while v.len() < len {
        v.push("none".into());
    }
    v
}

fn join_align(parts: &[String]) -> String {
    parts.join(",")
}

/// Build a fresh empty cell spec.
fn empty_cell_spec() -> NodeSpec {
    NodeSpec::Element {
        kind: "table_cell".into(),
        attrs: Attrs::new(),
        children: Vec::new(),
    }
}

/// Snapshot the doc's current `table` element into a `NodeSpec` so a
/// row/column structural edit can reinsert the whole subtree atomically.
/// We rebuild rather than poke at individual children because rows can
/// have different lengths after edits, and the alignment string lives on
/// the table itself — easiest to rewrite all three (table attrs + rows +
/// cells) in one shot.
fn clone_table_spec(doc: &Doc, table_key: NodeKey) -> Option<NodeSpec> {
    let table = doc.get_element(table_key)?;
    Some(NodeSpec::Element {
        kind: "table".into(),
        attrs: table.attrs.clone(),
        children: table
            .children
            .iter()
            .filter_map(|&row_key| {
                let row = doc.get_element(row_key)?;
                Some(NodeSpec::Element {
                    kind: "table_row".into(),
                    attrs: row.attrs.clone(),
                    children: row
                        .children
                        .iter()
                        .filter_map(|&c| {
                            let cell = doc.get_element(c)?;
                            Some(NodeSpec::Element {
                                kind: "table_cell".into(),
                                attrs: cell.attrs.clone(),
                                children: cell
                                    .children
                                    .iter()
                                    .filter_map(|&k| node_spec_from(doc, k))
                                    .collect(),
                            })
                        })
                        .collect(),
                })
            })
            .collect(),
    })
}

fn replace_table(
    ctx: &TableContext,
    new_table: NodeSpec,
    selection_cell_path: Option<(usize, usize)>,
    doc: &Doc,
) -> Transaction {
    let predicted = doc.peek_next_key();
    let new_table_key = predicted;
    // Pre-walk the spec to learn each cell's eventual key. materialize_into
    // is depth-first: table → row → cell → cell-children. The traversal is
    // deterministic, so we can mirror it here to compute the absolute key
    // of the first cell of a given row.
    let mut next = new_table_key + 1; // first row sits at +1
    let mut target_cell_key: Option<NodeKey> = None;
    if let NodeSpec::Element { children: rows, .. } = &new_table {
        for (r_idx, row) in rows.iter().enumerate() {
            let row_key = next;
            next += 1;
            if let NodeSpec::Element {
                children: cells, ..
            } = row
            {
                for (c_idx, _cell) in cells.iter().enumerate() {
                    let cell_key = next;
                    if Some((r_idx, c_idx)) == selection_cell_path {
                        target_cell_key = Some(cell_key);
                    }
                    // Advance past the cell + the keys it allocates for
                    // its children. node_spec_from preserves cell content;
                    // walk the subtree size.
                    next += 1 + subtree_size_in_spec(_cell) as u64;
                }
            }
            let _ = row_key;
        }
    }
    let mut tr = Transaction::new()
        .step(Step::RemoveNodes {
            parent: doc.root,
            range: ctx.table_idx_in_root..ctx.table_idx_in_root + 1,
        })
        .step(Step::InsertNodes {
            parent: doc.root,
            index: ctx.table_idx_in_root,
            nodes: vec![new_table],
        });
    if let Some(cell_key) = target_cell_key {
        tr = tr.select(Selection::caret(Point::element(cell_key, 0)));
    }
    tr
}

/// Count how many fresh keys `materialize_into` will allocate for this
/// spec — used by [`replace_table`] to predict cell keys before the step
/// runs. Each node consumes one key plus the total of its children.
fn subtree_size_in_spec(spec: &NodeSpec) -> usize {
    match spec {
        NodeSpec::Text { .. } | NodeSpec::Decorator { .. } => 0,
        NodeSpec::Element { children, .. } => {
            children.iter().map(|c| 1 + subtree_size_in_spec(c)).sum()
        }
    }
}

fn rebuild_table_spec_from(
    doc: &Doc,
    ctx: &TableContext,
    mutate: impl FnOnce(&mut Vec<NodeSpec>, &mut Attrs),
) -> Option<NodeSpec> {
    let mut table_spec = clone_table_spec(doc, ctx.table_key)?;
    if let NodeSpec::Element {
        attrs, children, ..
    } = &mut table_spec
    {
        mutate(children, attrs);
    }
    Some(table_spec)
}

/// Insert an empty row immediately above the caret's row.
pub fn insert_row_above(state: &EditorState) -> Option<Transaction> {
    let ctx = table_context(state)?;
    let new_table = rebuild_table_spec_from(&state.doc, &ctx, |rows, _attrs| {
        let new_row = NodeSpec::Element {
            kind: "table_row".into(),
            attrs: Attrs::new(),
            children: (0..ctx.cols).map(|_| empty_cell_spec()).collect(),
        };
        rows.insert(ctx.row_idx, new_row);
    })?;
    Some(replace_table(
        &ctx,
        new_table,
        Some((ctx.row_idx, ctx.col_idx)),
        &state.doc,
    ))
}

/// Insert an empty row immediately below the caret's row.
pub fn insert_row_below(state: &EditorState) -> Option<Transaction> {
    let ctx = table_context(state)?;
    let new_table = rebuild_table_spec_from(&state.doc, &ctx, |rows, _attrs| {
        let new_row = NodeSpec::Element {
            kind: "table_row".into(),
            attrs: Attrs::new(),
            children: (0..ctx.cols).map(|_| empty_cell_spec()).collect(),
        };
        rows.insert(ctx.row_idx + 1, new_row);
    })?;
    Some(replace_table(
        &ctx,
        new_table,
        Some((ctx.row_idx + 1, ctx.col_idx)),
        &state.doc,
    ))
}

/// Insert an empty column immediately before the caret's column.
pub fn insert_column_before(state: &EditorState) -> Option<Transaction> {
    let ctx = table_context(state)?;
    let new_table = rebuild_table_spec_from(&state.doc, &ctx, |rows, attrs| {
        for row in rows.iter_mut() {
            if let NodeSpec::Element { children, .. } = row {
                let at = ctx.col_idx.min(children.len());
                children.insert(at, empty_cell_spec());
            }
        }
        let mut aligns = split_align(attrs, ctx.cols);
        let at = ctx.col_idx.min(aligns.len());
        aligns.insert(at, "none".into());
        attrs.insert("align", join_align(&aligns));
    })?;
    Some(replace_table(
        &ctx,
        new_table,
        Some((ctx.row_idx, ctx.col_idx)),
        &state.doc,
    ))
}

/// Insert an empty column immediately after the caret's column.
pub fn insert_column_after(state: &EditorState) -> Option<Transaction> {
    let ctx = table_context(state)?;
    let new_table = rebuild_table_spec_from(&state.doc, &ctx, |rows, attrs| {
        for row in rows.iter_mut() {
            if let NodeSpec::Element { children, .. } = row {
                let at = (ctx.col_idx + 1).min(children.len());
                children.insert(at, empty_cell_spec());
            }
        }
        let mut aligns = split_align(attrs, ctx.cols);
        let at = (ctx.col_idx + 1).min(aligns.len());
        aligns.insert(at, "none".into());
        attrs.insert("align", join_align(&aligns));
    })?;
    Some(replace_table(
        &ctx,
        new_table,
        Some((ctx.row_idx, ctx.col_idx + 1)),
        &state.doc,
    ))
}

/// Remove the caret's row. If it was the only row, the table itself is
/// removed.
pub fn delete_row(state: &EditorState) -> Option<Transaction> {
    let ctx = table_context(state)?;
    if ctx.rows <= 1 {
        return delete_table(state);
    }
    let new_table = rebuild_table_spec_from(&state.doc, &ctx, |rows, _attrs| {
        if ctx.row_idx < rows.len() {
            rows.remove(ctx.row_idx);
        }
        // If we just stripped the header row, promote the new first row
        // so the table still has a header — markdown tables require one.
        if let Some(NodeSpec::Element { attrs, .. }) = rows.first_mut() {
            attrs.insert("header", true);
        }
    })?;
    // Caret falls into the row that now sits where the deleted row was —
    // or the previous row if we removed the last one.
    let target_row = if ctx.row_idx >= ctx.rows - 1 {
        ctx.row_idx - 1
    } else {
        ctx.row_idx
    };
    let target_col = ctx.col_idx.min(ctx.cols.saturating_sub(1));
    Some(replace_table(
        &ctx,
        new_table,
        Some((target_row, target_col)),
        &state.doc,
    ))
}

/// Remove the caret's column. If it was the only column, the table is
/// removed.
pub fn delete_column(state: &EditorState) -> Option<Transaction> {
    let ctx = table_context(state)?;
    if ctx.cols <= 1 {
        return delete_table(state);
    }
    let new_table = rebuild_table_spec_from(&state.doc, &ctx, |rows, attrs| {
        for row in rows.iter_mut() {
            if let NodeSpec::Element { children, .. } = row {
                if ctx.col_idx < children.len() {
                    children.remove(ctx.col_idx);
                }
            }
        }
        let mut aligns = split_align(attrs, ctx.cols);
        if ctx.col_idx < aligns.len() {
            aligns.remove(ctx.col_idx);
        }
        if aligns.iter().all(|s| s == "none") {
            attrs.remove("align");
        } else {
            attrs.insert("align", join_align(&aligns));
        }
    })?;
    let target_col = if ctx.col_idx >= ctx.cols - 1 {
        ctx.col_idx - 1
    } else {
        ctx.col_idx
    };
    Some(replace_table(
        &ctx,
        new_table,
        Some((ctx.row_idx, target_col)),
        &state.doc,
    ))
}

/// Remove the entire table the caret sits in. Caret falls back to an
/// empty paragraph inserted in the table's place.
pub fn delete_table(state: &EditorState) -> Option<Transaction> {
    let ctx = table_context(state)?;
    let predicted = state.doc.peek_next_key();
    let tr = Transaction::new()
        .step(Step::RemoveNodes {
            parent: state.doc.root,
            range: ctx.table_idx_in_root..ctx.table_idx_in_root + 1,
        })
        .step(Step::InsertNodes {
            parent: state.doc.root,
            index: ctx.table_idx_in_root,
            nodes: vec![NodeSpec::Element {
                kind: "paragraph".into(),
                attrs: Attrs::new(),
                children: Vec::new(),
            }],
        })
        .select(Selection::caret(Point::element(predicted, 0)));
    Some(tr)
}

/// Move the caret to the next cell in document order. Tab from the last
/// cell appends a new row and lands the caret in its first cell.
pub fn move_to_next_cell(state: &EditorState) -> Option<Transaction> {
    let ctx = table_context(state)?;
    let table = state.doc.get_element(ctx.table_key)?;
    // Next cell within the same row, or first cell of the next row.
    let cur_row = state.doc.get_element(ctx.row_key)?;
    if ctx.col_idx + 1 < cur_row.children.len() {
        let next_cell = cur_row.children[ctx.col_idx + 1];
        return Some(Transaction::new().select(Selection::caret(Point::element(next_cell, 0))));
    }
    if ctx.row_idx + 1 < table.children.len() {
        let next_row = state.doc.get_element(table.children[ctx.row_idx + 1])?;
        let next_cell = *next_row.children.first()?;
        return Some(Transaction::new().select(Selection::caret(Point::element(next_cell, 0))));
    }
    // Past the last cell — append a new row and land in its first cell.
    let new_table = rebuild_table_spec_from(&state.doc, &ctx, |rows, _attrs| {
        let new_row = NodeSpec::Element {
            kind: "table_row".into(),
            attrs: Attrs::new(),
            children: (0..ctx.cols).map(|_| empty_cell_spec()).collect(),
        };
        rows.push(new_row);
    })?;
    Some(replace_table(
        &ctx,
        new_table,
        Some((ctx.rows, 0)),
        &state.doc,
    ))
}

/// Append an empty row to the end of `table_key`. The caret jumps to the
/// first cell of the new row. Operates by table key rather than caret —
/// the table's "+ row" affordance lives outside any cell so the caller
/// doesn't need to plant a selection first.
pub fn append_row(state: &EditorState, table_key: NodeKey) -> Option<Transaction> {
    let (root, idx_in_root) = state.doc.child_index(table_key)?;
    if root != state.doc.root {
        return None;
    }
    let table = state.doc.get_element(table_key)?;
    if table.kind != "table" {
        return None;
    }
    let cols = table
        .children
        .iter()
        .filter_map(|&k| state.doc.get_element(k))
        .map(|r| r.children.len())
        .max()
        .unwrap_or(1)
        .max(1);
    let rows = table.children.len();
    let mut new_spec = clone_table_spec(&state.doc, table_key)?;
    if let NodeSpec::Element {
        children: rows_v, ..
    } = &mut new_spec
    {
        rows_v.push(NodeSpec::Element {
            kind: "table_row".into(),
            attrs: Attrs::new(),
            children: (0..cols).map(|_| empty_cell_spec()).collect(),
        });
    }
    Some(replace_table_at(
        idx_in_root,
        new_spec,
        Some((rows, 0)),
        &state.doc,
    ))
}

/// Append an empty column to the right edge of `table_key`. The caret
/// jumps to the first cell of the new column.
pub fn append_column(state: &EditorState, table_key: NodeKey) -> Option<Transaction> {
    let (root, idx_in_root) = state.doc.child_index(table_key)?;
    if root != state.doc.root {
        return None;
    }
    let table = state.doc.get_element(table_key)?;
    if table.kind != "table" {
        return None;
    }
    let cols = table
        .children
        .iter()
        .filter_map(|&k| state.doc.get_element(k))
        .map(|r| r.children.len())
        .max()
        .unwrap_or(0);
    let mut new_spec = clone_table_spec(&state.doc, table_key)?;
    if let NodeSpec::Element {
        children: rows_v,
        attrs,
        ..
    } = &mut new_spec
    {
        for row in rows_v.iter_mut() {
            if let NodeSpec::Element { children, .. } = row {
                children.push(empty_cell_spec());
            }
        }
        let mut aligns = split_align(attrs, cols);
        aligns.push("none".into());
        if aligns.iter().all(|s| s == "none") {
            attrs.remove("align");
        } else {
            attrs.insert("align", join_align(&aligns));
        }
    }
    Some(replace_table_at(
        idx_in_root,
        new_spec,
        Some((0, cols)),
        &state.doc,
    ))
}

/// Empty the caret's cell — strips every child node. Caret lands at the
/// cell's start. Returns `None` when the cell is already empty.
pub fn clear_cell(state: &EditorState) -> Option<Transaction> {
    let ctx = table_context(state)?;
    let cell = state.doc.get_element(ctx.cell_key)?;
    if cell.children.is_empty() {
        return None;
    }
    let n = cell.children.len();
    Some(
        Transaction::new()
            .step(Step::RemoveNodes {
                parent: ctx.cell_key,
                range: 0..n,
            })
            .select(Selection::caret(Point::element(ctx.cell_key, 0))),
    )
}

/// Duplicate the caret's row. The new row sits immediately below the
/// original; caret follows into the duplicate's column.
pub fn duplicate_row(state: &EditorState) -> Option<Transaction> {
    let ctx = table_context(state)?;
    let new_table = rebuild_table_spec_from(&state.doc, &ctx, |rows, _attrs| {
        if let Some(NodeSpec::Element {
            attrs, children, ..
        }) = rows.get(ctx.row_idx).cloned()
        {
            // Header attr never duplicates — markdown allows exactly one
            // header row, so the copy stays a body row.
            let mut copy_attrs = attrs;
            copy_attrs.remove("header");
            rows.insert(
                ctx.row_idx + 1,
                NodeSpec::Element {
                    kind: "table_row".into(),
                    attrs: copy_attrs,
                    children,
                },
            );
        }
    })?;
    Some(replace_table(
        &ctx,
        new_table,
        Some((ctx.row_idx + 1, ctx.col_idx)),
        &state.doc,
    ))
}

/// Duplicate the caret's column. The new column sits immediately to the
/// right of the original; caret follows.
pub fn duplicate_column(state: &EditorState) -> Option<Transaction> {
    let ctx = table_context(state)?;
    let new_table = rebuild_table_spec_from(&state.doc, &ctx, |rows, attrs| {
        for row in rows.iter_mut() {
            if let NodeSpec::Element { children, .. } = row {
                if let Some(cell) = children.get(ctx.col_idx).cloned() {
                    let insert_at = (ctx.col_idx + 1).min(children.len());
                    children.insert(insert_at, cell);
                }
            }
        }
        let mut aligns = split_align(attrs, ctx.cols);
        let dup = aligns
            .get(ctx.col_idx)
            .cloned()
            .unwrap_or_else(|| "none".into());
        let at = (ctx.col_idx + 1).min(aligns.len());
        aligns.insert(at, dup);
        if aligns.iter().all(|s| s == "none") {
            attrs.remove("align");
        } else {
            attrs.insert("align", join_align(&aligns));
        }
    })?;
    Some(replace_table(
        &ctx,
        new_table,
        Some((ctx.row_idx, ctx.col_idx + 1)),
        &state.doc,
    ))
}

/// Caret-free variant of [`replace_table`] for commands that operate on a
/// specific table key (`append_row`, `append_column`) rather than on the
/// caret's enclosing table.
fn replace_table_at(
    table_idx_in_root: usize,
    new_table: NodeSpec,
    selection_cell_path: Option<(usize, usize)>,
    doc: &Doc,
) -> Transaction {
    let predicted = doc.peek_next_key();
    let new_table_key = predicted;
    let mut next = new_table_key + 1;
    let mut target_cell_key: Option<NodeKey> = None;
    if let NodeSpec::Element { children: rows, .. } = &new_table {
        for (r_idx, row) in rows.iter().enumerate() {
            next += 1; // row key
            if let NodeSpec::Element {
                children: cells, ..
            } = row
            {
                for (c_idx, cell) in cells.iter().enumerate() {
                    let cell_key = next;
                    if Some((r_idx, c_idx)) == selection_cell_path {
                        target_cell_key = Some(cell_key);
                    }
                    next += 1 + subtree_size_in_spec(cell) as u64;
                }
            }
        }
    }
    let mut tr = Transaction::new()
        .step(Step::RemoveNodes {
            parent: doc.root,
            range: table_idx_in_root..table_idx_in_root + 1,
        })
        .step(Step::InsertNodes {
            parent: doc.root,
            index: table_idx_in_root,
            nodes: vec![new_table],
        });
    if let Some(cell_key) = target_cell_key {
        tr = tr.select(Selection::caret(Point::element(cell_key, 0)));
    }
    tr
}

/// Move the caret to the previous cell. From the first cell of the table,
/// the command is a no-op (returns `None`).
pub fn move_to_prev_cell(state: &EditorState) -> Option<Transaction> {
    let ctx = table_context(state)?;
    if ctx.col_idx > 0 {
        let row = state.doc.get_element(ctx.row_key)?;
        let prev_cell = row.children[ctx.col_idx - 1];
        return Some(Transaction::new().select(Selection::caret(Point::element(prev_cell, 0))));
    }
    if ctx.row_idx > 0 {
        let table = state.doc.get_element(ctx.table_key)?;
        let prev_row = state.doc.get_element(table.children[ctx.row_idx - 1])?;
        let last_cell = *prev_row.children.last()?;
        return Some(Transaction::new().select(Selection::caret(Point::element(last_cell, 0))));
    }
    None
}
