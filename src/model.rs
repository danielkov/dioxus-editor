//! Document model.
//!
//! A `Doc` is a flat arena of nodes keyed by a monotonically increasing
//! `NodeKey`. Edits produce a new `Doc` by cloning the arena and patching the
//! affected entries — editor documents are small enough that whole-doc clone
//! is cheaper than carrying a persistent data structure. The arena layout
//! decouples tree shape (each element holds `Vec<NodeKey>` children) from
//! storage, which means selection points referencing a key survive sibling
//! reordering and structural edits.
//!
//! Three node kinds: `Element` (containers and blocks), `Text` (leaves
//! carrying a string + format bitmask), `Decorator` (atomic embedded
//! widgets — mentions, file chips, link cards). Schema metadata (inline-ness,
//! atomicity, rendering, markdown round-trip) is registered on the side via
//! [`crate::Schema`] so the model itself stays pluggable: adding a new
//! decorator kind is a schema registration, not a model change.

use std::collections::HashMap;

use crate::attrs::Attrs;
use crate::format::FormatBits;

pub type NodeKey = u64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Element(ElementNode),
    Text(TextNode),
    Decorator(DecoratorNode),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementNode {
    pub key: NodeKey,
    pub kind: String,
    pub attrs: Attrs,
    pub children: Vec<NodeKey>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextNode {
    pub key: NodeKey,
    pub text: String,
    pub format: FormatBits,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecoratorNode {
    pub key: NodeKey,
    pub kind: String,
    pub attrs: Attrs,
}

impl Node {
    pub fn key(&self) -> NodeKey {
        match self {
            Node::Element(e) => e.key,
            Node::Text(t) => t.key,
            Node::Decorator(d) => d.key,
        }
    }

    pub fn kind(&self) -> &str {
        match self {
            Node::Element(e) => &e.kind,
            Node::Text(_) => "#text",
            Node::Decorator(d) => &d.kind,
        }
    }

    pub fn as_element(&self) -> Option<&ElementNode> {
        match self {
            Node::Element(e) => Some(e),
            _ => None,
        }
    }
    pub fn as_text(&self) -> Option<&TextNode> {
        match self {
            Node::Text(t) => Some(t),
            _ => None,
        }
    }
    pub fn as_decorator(&self) -> Option<&DecoratorNode> {
        match self {
            Node::Decorator(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_element_mut(&mut self) -> Option<&mut ElementNode> {
        match self {
            Node::Element(e) => Some(e),
            _ => None,
        }
    }
    pub fn as_text_mut(&mut self) -> Option<&mut TextNode> {
        match self {
            Node::Text(t) => Some(t),
            _ => None,
        }
    }
    pub fn as_decorator_mut(&mut self) -> Option<&mut DecoratorNode> {
        match self {
            Node::Decorator(d) => Some(d),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Doc {
    pub(crate) root: NodeKey,
    pub(crate) nodes: HashMap<NodeKey, Node>,
    next_key: NodeKey,
    parents: HashMap<NodeKey, NodeKey>,
}

impl Doc {
    pub fn empty() -> Self {
        let mut nodes = HashMap::new();
        let root_key: NodeKey = 1;
        let para_key: NodeKey = 2;
        nodes.insert(
            root_key,
            Node::Element(ElementNode {
                key: root_key,
                kind: "doc".into(),
                attrs: Attrs::new(),
                children: vec![para_key],
            }),
        );
        nodes.insert(
            para_key,
            Node::Element(ElementNode {
                key: para_key,
                kind: "paragraph".into(),
                attrs: Attrs::new(),
                children: Vec::new(),
            }),
        );
        let mut parents = HashMap::new();
        parents.insert(para_key, root_key);
        Self {
            root: root_key,
            nodes,
            next_key: 3,
            parents,
        }
    }

    pub(crate) fn fresh_key(&mut self) -> NodeKey {
        let k = self.next_key;
        self.next_key += 1;
        k
    }

    /// Read the next key the arena will hand out without consuming it.
    /// Callers can use this to predict the key a not-yet-applied
    /// `InsertNodes` step will assign to a single root node — useful for
    /// pointing the post-transaction selection inside the freshly created
    /// node.
    pub(crate) fn peek_next_key(&self) -> NodeKey {
        self.next_key
    }

    /// Key of the document's root element.
    pub fn root_key(&self) -> NodeKey {
        self.root
    }

    /// Read-only access to the document arena.
    pub fn nodes(&self) -> &HashMap<NodeKey, Node> {
        &self.nodes
    }

    pub fn get(&self, key: NodeKey) -> Option<&Node> {
        self.nodes.get(&key)
    }

    pub fn get_element(&self, key: NodeKey) -> Option<&ElementNode> {
        self.get(key)?.as_element()
    }

    pub fn get_text(&self, key: NodeKey) -> Option<&TextNode> {
        self.get(key)?.as_text()
    }

    pub fn get_decorator(&self, key: NodeKey) -> Option<&DecoratorNode> {
        self.get(key)?.as_decorator()
    }

    pub fn parent(&self, key: NodeKey) -> Option<NodeKey> {
        self.parents.get(&key).copied()
    }

    pub(crate) fn set_parent(&mut self, child: NodeKey, parent: NodeKey) {
        self.parents.insert(child, parent);
    }

    pub(crate) fn clear_parent(&mut self, child: NodeKey) {
        self.parents.remove(&child);
    }

    pub fn root_node(&self) -> &ElementNode {
        self.get_element(self.root).expect("root element missing")
    }

    pub fn child_index(&self, key: NodeKey) -> Option<(NodeKey, usize)> {
        let parent = self.parent(key)?;
        let pn = self.get_element(parent)?;
        let idx = pn.children.iter().position(|&k| k == key)?;
        Some((parent, idx))
    }

    /// Walk the immediate children of an element. Cheap iterator over key+node
    /// pairs.
    pub fn children_of<'a>(
        &'a self,
        key: NodeKey,
    ) -> impl Iterator<Item = (NodeKey, &'a Node)> + 'a {
        let kids = self
            .get_element(key)
            .map(|e| e.children.as_slice())
            .unwrap_or(&[]);
        kids.iter()
            .filter_map(move |&k| self.get(k).map(|n| (k, n)))
    }

    /// Total character count in a text-bearing node (text length for text
    /// nodes, 1 for atom decorators, 0 for elements).
    pub fn content_len(&self, key: NodeKey) -> usize {
        match self.get(key) {
            Some(Node::Text(t)) => t.text.chars().count(),
            Some(Node::Decorator(_)) => 1,
            _ => 0,
        }
    }

    /// Document is "empty" when the root has exactly one paragraph child and
    /// that paragraph has no children. Used by the view to render a
    /// placeholder.
    pub fn is_empty(&self) -> bool {
        let root = self.root_node();
        if root.children.len() != 1 {
            return false;
        }
        let only = root.children[0];
        match self.get(only) {
            Some(Node::Element(e)) => e.kind == "paragraph" && e.children.is_empty(),
            _ => false,
        }
    }
}

impl Default for Doc {
    fn default() -> Self {
        Self::empty()
    }
}
