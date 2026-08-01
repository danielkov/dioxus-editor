//! Steps and transactions.
//!
//! Every mutation of editor state goes through a [`Transaction`] — a sequence
//! of [`Step`]s plus an optional target selection. Steps are the *only* way
//! to change a `Doc`. This is the same idea as ProseMirror's transform
//! pipeline: atomic and reviewable.
//!
//! The step set is intentionally small. Higher-level operations (e.g.
//! "type a character", "split block at caret") are implemented as
//! command functions that compose primitives — they live in
//! [`crate::commands`]. Keeping the step vocabulary closed means undo only
//! has to invert a handful of cases.

use std::ops::Range;

use crate::attrs::{AttrValue, Attrs};
use crate::format::FormatBits;
use crate::model::{DecoratorNode, Doc, ElementNode, Node, NodeKey, TextNode};
use crate::selection::Selection;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepError {
    NoSuchNode(NodeKey),
    NotText(NodeKey),
    NotElement(NodeKey),
    OutOfRange { key: NodeKey, offset: usize },
    SchemaViolation(String),
}

impl std::fmt::Display for StepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchNode(key) => write!(f, "node {key} does not exist"),
            Self::NotText(key) => write!(f, "node {key} is not text"),
            Self::NotElement(key) => write!(f, "node {key} is not an element"),
            Self::OutOfRange { key, offset } => {
                write!(f, "offset {offset} is out of range for node {key}")
            }
            Self::SchemaViolation(message) => write!(f, "schema violation: {message}"),
        }
    }
}

impl std::error::Error for StepError {}

/// A node spec used by `Insert*` steps. The doc assigns the actual `NodeKey`
/// on insertion, so callers don't have to pre-allocate keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeSpec {
    Element {
        kind: String,
        attrs: Attrs,
        children: Vec<NodeSpec>,
    },
    Text {
        text: String,
        format: FormatBits,
    },
    Decorator {
        kind: String,
        attrs: Attrs,
    },
}

impl NodeSpec {
    pub fn paragraph(children: Vec<NodeSpec>) -> Self {
        Self::Element {
            kind: "paragraph".into(),
            attrs: Attrs::new(),
            children,
        }
    }
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            format: FormatBits::NONE,
        }
    }
    pub fn text_formatted(text: impl Into<String>, format: FormatBits) -> Self {
        Self::Text {
            text: text.into(),
            format,
        }
    }
    pub fn decorator(kind: impl Into<String>, attrs: Attrs) -> Self {
        Self::Decorator {
            kind: kind.into(),
            attrs,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Step {
    /// Replace `from..to` (character offsets, inclusive of `from`, exclusive
    /// of `to`) inside the text node `key` with `text`.
    ReplaceText {
        key: NodeKey,
        from: usize,
        to: usize,
        text: String,
    },
    /// Insert `nodes` as children of `parent` starting at child index
    /// `index`. The doc allocates fresh keys for every inserted node and
    /// updates the parent map.
    InsertNodes {
        parent: NodeKey,
        index: usize,
        nodes: Vec<NodeSpec>,
    },
    /// Remove children of `parent` in `range` (child indices). Removed
    /// subtrees are dropped from the arena.
    RemoveNodes {
        parent: NodeKey,
        range: Range<usize>,
    },
    /// Split a text node at character offset `at` into two adjacent text
    /// nodes that share parent + format.
    SplitText { key: NodeKey, at: usize },
    /// Split element `key` at child index `at`: children before `at` stay
    /// in the original; children at/after `at` move to a new sibling
    /// element of the same `kind`/`attrs`, inserted immediately after.
    SplitElement { key: NodeKey, at: usize },
    /// Merge two adjacent text nodes with identical format.
    JoinText { left: NodeKey, right: NodeKey },
    /// Set an attribute on an element or decorator. `None` removes it.
    SetAttr {
        key: NodeKey,
        name: String,
        value: Option<AttrValue>,
    },
    /// Replace the `kind` (and optionally attrs) of an element node.
    /// Children are preserved. Used by `Enter` inside a heading to
    /// promote the new sibling block to a paragraph, etc.
    SetElementKind {
        key: NodeKey,
        kind: String,
        attrs: Option<Attrs>,
    },
    /// Set the entire format bitfield of a text node (typically used after
    /// toggling marks across a range — higher-level code splits text nodes
    /// first so the range matches a single node).
    SetFormat { key: NodeKey, format: FormatBits },
    /// Replace the entire `Doc` arena wholesale. Used exclusively by the
    /// undo/redo path so a restored snapshot preserves its original
    /// `NodeKey`s — that way the snapshot's recorded selection still
    /// references live nodes after the replay (rebuilding via remove +
    /// insert would allocate fresh keys and leave the selection dangling).
    ReplaceDoc(Doc),
}

impl Step {
    pub fn apply(&self, mut doc: Doc) -> Result<Doc, StepError> {
        match self {
            Step::ReplaceText {
                key,
                from,
                to,
                text,
            } => {
                let node = doc.nodes.get_mut(key).ok_or(StepError::NoSuchNode(*key))?;
                let t = node.as_text_mut().ok_or(StepError::NotText(*key))?;
                let (b_from, b_to) =
                    char_byte_range(&t.text, *from, *to).ok_or(StepError::OutOfRange {
                        key: *key,
                        offset: *to,
                    })?;
                t.text.replace_range(b_from..b_to, text);
                Ok(doc)
            }
            Step::InsertNodes {
                parent,
                index,
                nodes,
            } => {
                // Pre-allocate keys for the whole subtree forest.
                let parent_key = *parent;
                let pelem = doc
                    .get_element(parent_key)
                    .ok_or(StepError::NotElement(parent_key))?;
                if *index > pelem.children.len() {
                    return Err(StepError::OutOfRange {
                        key: parent_key,
                        offset: *index,
                    });
                }
                let mut new_keys = Vec::with_capacity(nodes.len());
                for spec in nodes {
                    let key = materialize_into(&mut doc, parent_key, spec)?;
                    new_keys.push(key);
                }
                // Insert into the parent's children.
                let pelem = doc
                    .nodes
                    .get_mut(&parent_key)
                    .and_then(Node::as_element_mut);
                if let Some(pe) = pelem {
                    let mut tail = pe.children.split_off(*index);
                    pe.children.extend(new_keys.iter().copied());
                    pe.children.append(&mut tail);
                }
                Ok(doc)
            }
            Step::RemoveNodes { parent, range } => {
                let parent_key = *parent;
                let pe = doc
                    .nodes
                    .get_mut(&parent_key)
                    .and_then(Node::as_element_mut)
                    .ok_or(StepError::NotElement(parent_key))?;
                if range.end > pe.children.len() || range.start > range.end {
                    return Err(StepError::OutOfRange {
                        key: parent_key,
                        offset: range.end,
                    });
                }
                let removed: Vec<NodeKey> = pe.children.drain(range.clone()).collect();
                for r in removed {
                    drop_subtree(&mut doc, r);
                }
                Ok(doc)
            }
            Step::SplitText { key, at } => {
                let t = doc
                    .nodes
                    .get(key)
                    .and_then(Node::as_text)
                    .ok_or(StepError::NotText(*key))?
                    .clone();
                let (parent, idx) = doc.child_index(*key).ok_or(StepError::NoSuchNode(*key))?;
                let bytes = char_byte_offset(&t.text, *at).ok_or(StepError::OutOfRange {
                    key: *key,
                    offset: *at,
                })?;
                let left_str = t.text[..bytes].to_string();
                let right_str = t.text[bytes..].to_string();

                // Patch original to left half.
                {
                    let mt = doc.nodes.get_mut(key).and_then(Node::as_text_mut).unwrap();
                    mt.text = left_str;
                }
                // Insert right half as a sibling.
                let right_key = doc.fresh_key();
                doc.nodes.insert(
                    right_key,
                    Node::Text(TextNode {
                        key: right_key,
                        text: right_str,
                        format: t.format,
                    }),
                );
                doc.set_parent(right_key, parent);
                if let Some(pe) = doc.nodes.get_mut(&parent).and_then(Node::as_element_mut) {
                    pe.children.insert(idx + 1, right_key);
                }
                Ok(doc)
            }
            Step::SplitElement { key, at } => {
                let elem = doc
                    .get_element(*key)
                    .ok_or(StepError::NotElement(*key))?
                    .clone();
                if *at > elem.children.len() {
                    return Err(StepError::OutOfRange {
                        key: *key,
                        offset: *at,
                    });
                }
                let (gp, idx) = doc.child_index(*key).ok_or(StepError::NoSuchNode(*key))?;
                let tail: Vec<NodeKey> = {
                    let me = doc
                        .nodes
                        .get_mut(key)
                        .and_then(Node::as_element_mut)
                        .unwrap();
                    me.children.split_off(*at)
                };
                let new_key = doc.fresh_key();
                doc.nodes.insert(
                    new_key,
                    Node::Element(ElementNode {
                        key: new_key,
                        kind: elem.kind.clone(),
                        attrs: elem.attrs.clone(),
                        children: tail.clone(),
                    }),
                );
                for &c in &tail {
                    doc.set_parent(c, new_key);
                }
                doc.set_parent(new_key, gp);
                if let Some(pe) = doc.nodes.get_mut(&gp).and_then(Node::as_element_mut) {
                    pe.children.insert(idx + 1, new_key);
                }
                Ok(doc)
            }
            Step::JoinText { left, right } => {
                if left == right {
                    return Err(StepError::SchemaViolation(
                        "cannot join a text node with itself".into(),
                    ));
                }
                let r = doc
                    .nodes
                    .get(right)
                    .and_then(Node::as_text)
                    .ok_or(StepError::NotText(*right))?
                    .clone();
                let l = doc
                    .nodes
                    .get(left)
                    .and_then(Node::as_text)
                    .ok_or(StepError::NotText(*left))?;
                if l.format != r.format {
                    return Err(StepError::SchemaViolation(
                        "join text nodes with different formats".into(),
                    ));
                }
                let (left_parent, left_idx) =
                    doc.child_index(*left).ok_or(StepError::NoSuchNode(*left))?;
                let (right_parent, right_idx) = doc
                    .child_index(*right)
                    .ok_or(StepError::NoSuchNode(*right))?;
                if left_parent != right_parent || right_idx != left_idx + 1 {
                    return Err(StepError::SchemaViolation(
                        "join text nodes must be adjacent siblings".into(),
                    ));
                }

                doc.nodes
                    .get_mut(left)
                    .and_then(Node::as_text_mut)
                    .expect("text validated above")
                    .text
                    .push_str(&r.text);
                doc.nodes
                    .get_mut(&left_parent)
                    .and_then(Node::as_element_mut)
                    .expect("parent validated above")
                    .children
                    .remove(right_idx);
                drop_subtree(&mut doc, *right);
                Ok(doc)
            }
            Step::SetAttr { key, name, value } => {
                let node = doc.nodes.get_mut(key).ok_or(StepError::NoSuchNode(*key))?;
                match node {
                    Node::Element(e) => match value {
                        Some(v) => e.attrs.insert(name.clone(), v.clone()),
                        None => {
                            e.attrs.remove(name);
                        }
                    },
                    Node::Decorator(d) => match value {
                        Some(v) => d.attrs.insert(name.clone(), v.clone()),
                        None => {
                            d.attrs.remove(name);
                        }
                    },
                    Node::Text(_) => return Err(StepError::NotElement(*key)),
                }
                Ok(doc)
            }
            Step::SetElementKind { key, kind, attrs } => {
                let e = doc
                    .nodes
                    .get_mut(key)
                    .and_then(Node::as_element_mut)
                    .ok_or(StepError::NotElement(*key))?;
                e.kind = kind.clone();
                if let Some(a) = attrs {
                    e.attrs = a.clone();
                }
                Ok(doc)
            }
            Step::SetFormat { key, format } => {
                let t = doc
                    .nodes
                    .get_mut(key)
                    .and_then(Node::as_text_mut)
                    .ok_or(StepError::NotText(*key))?;
                t.format = *format;
                Ok(doc)
            }
            Step::ReplaceDoc(snapshot) => Ok(snapshot.clone()),
        }
    }
}

fn materialize_into(doc: &mut Doc, parent: NodeKey, spec: &NodeSpec) -> Result<NodeKey, StepError> {
    let key = doc.fresh_key();
    match spec {
        NodeSpec::Element {
            kind,
            attrs,
            children,
        } => {
            doc.nodes.insert(
                key,
                Node::Element(ElementNode {
                    key,
                    kind: kind.clone(),
                    attrs: attrs.clone(),
                    children: Vec::with_capacity(children.len()),
                }),
            );
            doc.set_parent(key, parent);
            for child_spec in children {
                let child_key = materialize_into(doc, key, child_spec)?;
                if let Some(e) = doc.nodes.get_mut(&key).and_then(Node::as_element_mut) {
                    e.children.push(child_key);
                }
            }
        }
        NodeSpec::Text { text, format } => {
            doc.nodes.insert(
                key,
                Node::Text(TextNode {
                    key,
                    text: text.clone(),
                    format: *format,
                }),
            );
            doc.set_parent(key, parent);
        }
        NodeSpec::Decorator { kind, attrs } => {
            doc.nodes.insert(
                key,
                Node::Decorator(DecoratorNode {
                    key,
                    kind: kind.clone(),
                    attrs: attrs.clone(),
                }),
            );
            doc.set_parent(key, parent);
        }
    }
    Ok(key)
}

fn drop_subtree(doc: &mut Doc, root: NodeKey) {
    let mut stack = vec![root];
    while let Some(k) = stack.pop() {
        if let Some(Node::Element(e)) = doc.nodes.get(&k) {
            stack.extend(e.children.iter().copied());
        }
        doc.nodes.remove(&k);
        doc.clear_parent(k);
    }
}

fn char_byte_offset(s: &str, char_idx: usize) -> Option<usize> {
    if char_idx == 0 {
        return Some(0);
    }
    let mut count = 0;
    for (b, _) in s.char_indices() {
        if count == char_idx {
            return Some(b);
        }
        count += 1;
    }
    if count == char_idx {
        Some(s.len())
    } else {
        None
    }
}

fn char_byte_range(s: &str, from: usize, to: usize) -> Option<(usize, usize)> {
    if from > to {
        return None;
    }
    let b_from = char_byte_offset(s, from)?;
    let b_to = char_byte_offset(s, to)?;
    Some((b_from, b_to))
}

#[derive(Clone, Debug, Default)]
pub struct Transaction {
    pub steps: Vec<Step>,
    /// Final selection after the transaction. If `None`, the editor keeps
    /// the previous selection (mapped through the steps).
    pub selection: Option<Selection>,
    /// New value for `EditorState.pending_format` after the transaction
    /// applies. `Some(bits)` overrides; `None` carries the prior value
    /// forward. Used by mark-toggle commands on a collapsed selection to
    /// queue a format for the next typed character.
    pub pending_format: Option<crate::format::FormatBits>,
    /// Per-transaction metadata used by plugins to flag intent ("from undo",
    /// "from input rule", "from paste") so they don't re-fire.
    pub meta: std::collections::HashMap<String, String>,
}

impl Transaction {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn step(mut self, step: Step) -> Self {
        self.steps.push(step);
        self
    }

    pub fn select(mut self, sel: Selection) -> Self {
        self.selection = Some(sel);
        self
    }

    pub fn pending_format(mut self, fmt: crate::format::FormatBits) -> Self {
        self.pending_format = Some(fmt);
        self
    }

    pub fn meta(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.meta.insert(key.into(), value.into());
        self
    }

    pub fn get_meta(&self, key: &str) -> Option<&str> {
        self.meta.get(key).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty() && self.selection.is_none() && self.pending_format.is_none()
    }

    pub fn apply(self, mut doc: Doc) -> Result<(Doc, Option<Selection>), StepError> {
        for step in self.steps {
            doc = step.apply(doc)?;
        }
        Ok((doc, self.selection))
    }
}
