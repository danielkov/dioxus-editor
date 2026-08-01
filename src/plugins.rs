//! Built-in plugins.
//!
//! Each plugin lives behind a small struct that implements
//! [`crate::plugin::Plugin`]. The set bundled here covers the common
//! editing affordances rich-text editors commonly need — markdown shortcuts,
//! undo/redo history, default keymap. Host crates append more (mention
//! picker, slash command palette, link preview) by constructing a plugin
//! that fits the same trait.

use crate::attrs::Attrs;
use crate::commands::{
    delete_backward, delete_forward, split_block, toggle_bold, toggle_code, toggle_italic,
    toggle_strike,
};
use crate::format::FormatBits;
use crate::model::{Doc, Node, NodeKey};
use crate::plugin::{Command, EditorEvent, KeyBinding, Plugin};
use crate::selection::{Point, PointKind, Selection};
use crate::state::EditorState;
use crate::step::{NodeSpec, Step, Transaction};

// -- keymap ---------------------------------------------------------------

/// Default keymap: bold/italic/strike/code via Cmd-/Ctrl- shortcuts,
/// Backspace/Delete, Enter for split.
pub struct DefaultKeymap;

impl Plugin for DefaultKeymap {
    fn keymap(&self) -> Vec<KeyBinding> {
        vec![
            KeyBinding {
                keys: "Mod-b".into(),
                command: toggle_bold as Command,
            },
            KeyBinding {
                keys: "Mod-i".into(),
                command: toggle_italic as Command,
            },
            KeyBinding {
                keys: "Mod-Shift-s".into(),
                command: toggle_strike as Command,
            },
            KeyBinding {
                keys: "Mod-e".into(),
                command: toggle_code as Command,
            },
            KeyBinding {
                keys: "Backspace".into(),
                command: delete_backward as Command,
            },
            KeyBinding {
                keys: "Delete".into(),
                command: delete_forward as Command,
            },
            KeyBinding {
                keys: "Shift-Enter".into(),
                command: split_block as Command,
            },
        ]
    }
}

// -- history --------------------------------------------------------------

/// Linear undo stack of doc snapshots. Each user-facing edit pushes the
/// pre-state once; consecutive single-character typing into the same
/// text node merges into the same undo group so the user doesn't have to
/// press undo once per character.
///
/// Trigger history navigation with [`EditorEvent::Undo`] and
/// [`EditorEvent::Redo`]. The resulting transaction is tagged
/// `origin=history` so it doesn't churn the stack.
pub struct History {
    past: Vec<EditorState>,
    future: Vec<EditorState>,
    /// True while consecutive transactions are typing — used so a burst
    /// of typed characters collapses into one undo entry.
    in_typing_group: bool,
    /// `(text_key, caret_offset_after_insert)` for the most recent
    /// in-group typing insertion. The next insertion must be at the
    /// immediate-right of this position (same node, offset matches);
    /// otherwise the user has moved the caret and the group breaks.
    last_typing_pos: Option<(NodeKey, usize)>,
    limit: usize,
}

impl History {
    pub fn new() -> Self {
        Self {
            past: Vec::new(),
            future: Vec::new(),
            in_typing_group: false,
            last_typing_pos: None,
            limit: 200,
        }
    }
}

/// Return the `(text_key, end_offset)` of a typing transaction — the
/// position where the caret sits *after* the insert. Reads from
/// `tr.selection` so it works equally for ReplaceText (extends an
/// existing text node) and InsertNodes (allocates a fresh one).
fn typing_position(tr: &Transaction) -> Option<(NodeKey, usize)> {
    match &tr.selection {
        Some(crate::selection::Selection::Range { anchor, focus })
            if anchor == focus && anchor.kind == crate::selection::PointKind::Text =>
        {
            Some((anchor.key, anchor.offset))
        }
        _ => None,
    }
}

/// Whether a transaction looks like the user is "typing" — a single
/// text-producing edit (either a `ReplaceText` insert into an existing
/// text node, or an `InsertNodes` of one fresh text node), possibly with
/// a trailing selection update. Anything else (deletes, format toggles,
/// structural changes, block transforms) closes the typing group so the
/// next typing burst gets its own undo entry.
fn is_typing(tr: &Transaction) -> bool {
    let mut saw_insert = false;
    for step in &tr.steps {
        match step {
            Step::ReplaceText { from, to, text, .. } if from == to && !text.is_empty() => {
                saw_insert = true;
            }
            Step::InsertNodes { nodes, .. }
                if nodes.len() == 1
                    && matches!(&nodes[0], NodeSpec::Text { text, .. } if !text.is_empty()) =>
            {
                saw_insert = true;
            }
            _ => return false,
        }
    }
    saw_insert
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for History {
    fn handle_event(&mut self, state: &EditorState, event: &EditorEvent) -> Option<Transaction> {
        match event {
            EditorEvent::Undo => {
                let prev = self.past.pop()?;
                self.future.push(state.clone());
                self.in_typing_group = false;
                Some(replay_transaction(state, &prev))
            }
            EditorEvent::Redo => {
                let next = self.future.pop()?;
                self.past.push(state.clone());
                self.in_typing_group = false;
                Some(replay_transaction(state, &next))
            }
            EditorEvent::SelectionChange(_) => None,
        }
    }

    fn append_transaction(
        &mut self,
        tr: &Transaction,
        old: &EditorState,
        _new: &EditorState,
    ) -> Option<Transaction> {
        if tr.get_meta("origin") == Some("history") {
            return None;
        }
        if tr.steps.is_empty() {
            return None;
        }

        let typing = is_typing(tr);
        let typing_pos = if typing { typing_position(tr) } else { None };
        // The burst continues only if the new insertion is at the
        // immediate-right of the previous one (same node, offset N+
        // chars_typed_last). Anything else — user clicked, used arrow
        // keys, the caret jumped — breaks the burst so the next undo
        // doesn't unwind across an unrelated edit site.
        let adjacent_to_last = match (typing_pos, self.last_typing_pos) {
            (Some((curr_key, curr_off)), Some((prev_key, prev_off))) => {
                curr_key == prev_key && curr_off == prev_off + 1
            }
            _ => false,
        };
        if typing && self.in_typing_group && adjacent_to_last {
            // Continue an in-flight typing burst.
            self.future.clear();
            self.last_typing_pos = typing_pos.or(self.last_typing_pos);
            return None;
        }
        self.past.push(old.clone());
        if self.past.len() > self.limit {
            self.past.remove(0);
        }
        self.in_typing_group = typing;
        self.last_typing_pos = typing_pos;
        self.future.clear();
        None
    }
}

/// Build a transaction that swaps the live `Doc` for a previously-
/// recorded snapshot. Using `ReplaceDoc` preserves the snapshot's
/// original `NodeKey`s so the snapshot's recorded selection still points
/// at live nodes after the replay — rebuilding via remove + insert
/// would allocate fresh keys and the restored caret would dangle (the
/// user-visible symptom: caret jumps to start of doc after undo).
fn replay_transaction(_current: &EditorState, snapshot: &EditorState) -> Transaction {
    Transaction::new()
        .meta("origin", "history")
        .step(Step::ReplaceDoc(snapshot.doc.clone()))
        .select(snapshot.selection.clone())
}

fn node_spec_from(doc: &Doc, key: NodeKey) -> Option<crate::step::NodeSpec> {
    use crate::step::NodeSpec;
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

// -- input rules ----------------------------------------------------------

struct InlineRule {
    open: &'static str,
    close: &'static str,
    format: FormatBits,
}

/// Markdown-flavored input rules. After every text-inserting transaction
/// the plugin scans the touched text node for a paired open/close
/// delimiter that ends at the caret. A match rewrites the affected text
/// node into three text nodes — leading plain, formatted body, trailing
/// plain — preserving surrounding format bits and dropping the markers.
///
/// The rule fires on the *closing* keystroke (the second `*` of `**bold**`)
/// so it never alters intermediate typing.
pub struct MarkdownShortcuts {
    inline_rules: Vec<InlineRule>,
}

impl MarkdownShortcuts {
    pub fn new() -> Self {
        Self {
            inline_rules: vec![
                InlineRule {
                    open: "**",
                    close: "**",
                    format: FormatBits::BOLD,
                },
                InlineRule {
                    open: "~~",
                    close: "~~",
                    format: FormatBits::STRIKE,
                },
                InlineRule {
                    open: "_",
                    close: "_",
                    format: FormatBits::ITALIC,
                },
                InlineRule {
                    open: "`",
                    close: "`",
                    format: FormatBits::CODE,
                },
            ],
        }
    }
}

impl Default for MarkdownShortcuts {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for MarkdownShortcuts {
    fn append_transaction(
        &mut self,
        tr: &Transaction,
        _old: &EditorState,
        new: &EditorState,
    ) -> Option<Transaction> {
        if tr.get_meta("origin") == Some("input-rule") {
            return None;
        }
        // Find the last single-character text insertion in the transaction
        // — that's the keystroke we react to.
        let last_insert = tr.steps.iter().rev().find_map(|s| match s {
            Step::ReplaceText {
                key,
                from,
                to,
                text,
            } if from == to && text.chars().count() == 1 => Some((*key, *from + 1, text.clone())),
            _ => None,
        })?;
        let (text_key, caret, inserted_char) = last_insert;
        // Inside a code block, all input is literal — markdown shortcuts
        // (block or inline) must not fire.
        if containing_block_is_raw(&new.doc, text_key) {
            return None;
        }
        // Block-level shortcuts trigger on the closing space character of
        // the pattern (`# `, `> `, `- `, `1. ` …). Check those first since
        // they consume an entire prefix and we'd rather not waste cycles
        // looking for an inline match in the same span.
        if inserted_char == " " {
            if let Some(tr) = apply_block_rule(&new.doc, text_key, caret) {
                return Some(tr);
            }
        }
        if inserted_char == ")" && new.schema.has_decorator("link") {
            if let Some(tr) = apply_link_rule(&new.doc, text_key, caret) {
                return Some(tr);
            }
        }
        for rule in &self.inline_rules {
            if let Some(tr) = apply_inline_rule(&new.doc, text_key, caret, rule) {
                return Some(tr);
            }
        }
        None
    }
}

/// True when `text_key` sits inside a fenced/raw block (code_block), where
/// markdown shortcuts must not fire because the contents are meant to be
/// taken literally.
fn containing_block_is_raw(doc: &Doc, text_key: NodeKey) -> bool {
    let mut cur = text_key;
    while let Some(parent) = doc.parent(cur) {
        if let Some(e) = doc.get_element(parent) {
            if e.kind == "code_block" {
                return true;
            }
        }
        if parent == doc.root {
            return false;
        }
        cur = parent;
    }
    false
}

fn apply_block_rule(doc: &Doc, text_key: NodeKey, caret_chars: usize) -> Option<Transaction> {
    // The matched range starts at the head of the text node — so the rule
    // only fires when the user types the pattern at the very beginning of
    // a paragraph. This avoids `# ` mid-sentence triggering H1.
    let t = doc.get_text(text_key)?;
    let prefix: String = t.text.chars().take(caret_chars).collect();
    let (consume, new_kind, level): (usize, &'static str, Option<i64>) = if prefix == "# " {
        (2, "heading", Some(1))
    } else if prefix == "## " {
        (3, "heading", Some(2))
    } else if prefix == "### " {
        (4, "heading", Some(3))
    } else if prefix == "> " {
        (2, "blockquote", None)
    } else if prefix == "- " || prefix == "* " {
        (2, "bullet_list", None)
    } else if prefix == "1. " {
        (3, "ordered_list", None)
    } else {
        return None;
    };

    // The text node must be the FIRST child of a block directly under doc.root.
    let (block_key, child_idx_in_block) = doc.child_index(text_key)?;
    if child_idx_in_block != 0 {
        return None;
    }
    let block = doc.get_element(block_key)?;
    if block.kind != "paragraph" {
        return None;
    }
    let (root_parent, idx_in_root) = doc.child_index(block_key)?;
    if root_parent != doc.root {
        return None;
    }

    // Build new block's children: same as original but with the prefix
    // stripped from the first text node.
    let mut new_children: Vec<NodeSpec> = Vec::new();
    for (i, &k) in block.children.iter().enumerate() {
        match doc.get(k)? {
            Node::Text(tn) if i == 0 => {
                let stripped: String = tn.text.chars().skip(consume).collect();
                new_children.push(NodeSpec::Text {
                    text: stripped,
                    format: tn.format,
                });
            }
            Node::Text(tn) => {
                new_children.push(NodeSpec::Text {
                    text: tn.text.clone(),
                    format: tn.format,
                });
            }
            Node::Decorator(d) => {
                new_children.push(NodeSpec::Decorator {
                    kind: d.kind.clone(),
                    attrs: d.attrs.clone(),
                });
            }
            Node::Element(e) => {
                new_children.push(NodeSpec::Element {
                    kind: e.kind.clone(),
                    attrs: e.attrs.clone(),
                    children: e
                        .children
                        .iter()
                        .filter_map(|&c| node_spec_from(doc, c))
                        .collect(),
                });
            }
        }
    }

    // List shortcuts wrap inside list_item inside list.
    let new_spec = match new_kind {
        "bullet_list" | "ordered_list" => NodeSpec::Element {
            kind: new_kind.into(),
            attrs: Attrs::new(),
            children: vec![NodeSpec::Element {
                kind: "list_item".into(),
                attrs: Attrs::new(),
                children: new_children,
            }],
        },
        _ => {
            let mut attrs = Attrs::new();
            if let Some(l) = level {
                attrs.insert("level", l);
            }
            NodeSpec::Element {
                kind: new_kind.into(),
                attrs,
                children: new_children,
            }
        }
    };

    let predicted = doc.peek_next_key();
    // For paragraph-shaped wrappers (heading/blockquote), the first text
    // child sits one key after the new block. For lists, list_item is +1
    // and its first text is +2 — same logic, different label.
    let inner_text_key = match new_kind {
        "bullet_list" | "ordered_list" => predicted + 2,
        _ => predicted + 1,
    };

    let tr = Transaction::new()
        .step(Step::RemoveNodes {
            parent: doc.root,
            range: idx_in_root..idx_in_root + 1,
        })
        .step(Step::InsertNodes {
            parent: doc.root,
            index: idx_in_root,
            nodes: vec![new_spec],
        })
        .select(Selection::caret(Point::text(inner_text_key, 0)))
        .meta("origin", "input-rule");
    Some(tr)
}

fn apply_link_rule(doc: &Doc, text_key: NodeKey, caret_chars: usize) -> Option<Transaction> {
    let text = doc.get_text(text_key)?;
    let chars: Vec<char> = text.text.chars().collect();
    let close_paren = caret_chars.checked_sub(1)?;
    if chars.get(close_paren) != Some(&')') {
        return None;
    }

    let close_bracket = (1..close_paren)
        .rev()
        .find(|&index| chars[index - 1] == ']' && chars[index] == '(')?
        - 1;
    let open_bracket = (0..close_bracket)
        .rev()
        .find(|&index| chars[index] == '[')?;
    if open_bracket + 1 == close_bracket || close_bracket + 2 == close_paren {
        return None;
    }

    let href_chars = &chars[close_bracket + 2..close_paren];
    let mut depth = 0;
    let mut escaped = false;
    for &character in href_chars {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '(' => depth += 1,
            ')' if depth == 0 => return None,
            ')' => depth -= 1,
            _ => {}
        }
    }
    if escaped || depth != 0 {
        return None;
    }

    let label: String = chars[open_bracket + 1..close_bracket].iter().collect();
    let href: String = href_chars.iter().collect();
    let before: String = chars[..open_bracket].iter().collect();
    let after: String = chars[caret_chars..].iter().collect();
    let (parent, index) = doc.child_index(text_key)?;

    let mut nodes = Vec::new();
    if !before.is_empty() {
        nodes.push(NodeSpec::Text {
            text: before,
            format: text.format,
        });
    }
    nodes.push(NodeSpec::decorator(
        "link",
        Attrs::new().with("href", href).with("text", label),
    ));
    if !after.is_empty() {
        nodes.push(NodeSpec::Text {
            text: after,
            format: text.format,
        });
    }
    let caret = index + nodes.len();

    Some(
        Transaction::new()
            .step(Step::RemoveNodes {
                parent,
                range: index..index + 1,
            })
            .step(Step::InsertNodes {
                parent,
                index,
                nodes,
            })
            .select(Selection::caret(Point::element(parent, caret)))
            .meta("origin", "input-rule"),
    )
}

fn apply_inline_rule(
    doc: &Doc,
    text_key: NodeKey,
    caret_chars: usize,
    rule: &InlineRule,
) -> Option<Transaction> {
    let t = doc.get_text(text_key)?;
    let close_len = rule.close.chars().count();
    let open_len = rule.open.chars().count();
    if caret_chars < close_len + open_len + 1 {
        return None;
    }
    let close_start = caret_chars - close_len;
    let close_slice: String = t.text.chars().skip(close_start).take(close_len).collect();
    if close_slice != rule.close {
        return None;
    }
    // Find a matching open marker before the close. The body must be at
    // least one character.
    let upper = close_start.saturating_sub(open_len);
    let mut open_start: Option<usize> = None;
    for s in (0..=upper).rev() {
        let sample: String = t.text.chars().skip(s).take(open_len).collect();
        if sample == rule.open && s + open_len < close_start {
            open_start = Some(s);
            break;
        }
    }
    let open_start = open_start?;
    let body_start = open_start + open_len;
    let body_end = close_start;

    // Bail when the body is just whitespace — the user almost certainly
    // didn't mean to format a blank.
    let body: String = t
        .text
        .chars()
        .skip(body_start)
        .take(body_end - body_start)
        .collect();
    if body.trim().is_empty() {
        return None;
    }

    let pre: String = t.text.chars().take(open_start).collect();
    let post: String = t.text.chars().skip(close_start + close_len).collect();
    let base_format = t.format;
    let body_format = FormatBits(base_format.0 | rule.format.0);

    let (parent_key, child_idx) = doc.child_index(text_key)?;

    let mut nodes: Vec<NodeSpec> = Vec::new();
    let pre_present = !pre.is_empty();
    if pre_present {
        nodes.push(NodeSpec::Text {
            text: pre,
            format: base_format,
        });
    }
    nodes.push(NodeSpec::Text {
        text: body,
        format: body_format,
    });
    let post_present = !post.is_empty();
    if post_present {
        nodes.push(NodeSpec::Text {
            text: post,
            format: base_format,
        });
    }

    // The body node lives right after the optional pre node.
    let body_idx = child_idx + if pre_present { 1 } else { 0 };
    let after_body = body_idx + 1;

    let tr = Transaction::new()
        .step(Step::RemoveNodes {
            parent: parent_key,
            range: child_idx..child_idx + 1,
        })
        .step(Step::InsertNodes {
            parent: parent_key,
            index: child_idx,
            nodes,
        })
        .select(Selection::caret(Point::element(parent_key, after_body)))
        .meta("origin", "input-rule");

    Some(tr)
}

// -- view helper ----------------------------------------------------------

/// Last (key, kind, offset) leaf in the doc, used as a "click after
/// content" caret target by the view.
pub fn last_leaf(doc: &Doc) -> Option<(NodeKey, PointKind, usize)> {
    let root = doc.get_element(doc.root)?;
    let last_block = *root.children.last()?;
    let block = doc.get_element(last_block)?;
    if let Some(&last) = block.children.last() {
        match doc.get(last)? {
            Node::Text(t) => Some((t.key, PointKind::Text, t.text.chars().count())),
            Node::Decorator(d) => Some((d.key, PointKind::Element, 1)),
            Node::Element(e) => Some((e.key, PointKind::Element, e.children.len())),
        }
    } else {
        Some((last_block, PointKind::Element, 0))
    }
}
