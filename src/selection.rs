//! Selection model.
//!
//! Selection lives in the editor state, not the DOM — the DOM selection is
//! mirrored from the model after every render. Selection points reference
//! [`NodeKey`]s, so a transaction that splits or merges nodes can express
//! "what the selection should be afterwards" without first knowing how the
//! DOM will diff. The reconciler is responsible for mapping a model selection
//! to a DOM range and back.

use crate::model::NodeKey;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointKind {
    /// Offset is a character index inside a [`crate::Node::Text`]'s string.
    Text,
    /// Offset is a child index inside a [`crate::Node::Element`]'s children list.
    Element,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub key: NodeKey,
    pub offset: usize,
    pub kind: PointKind,
}

impl Point {
    pub fn text(key: NodeKey, offset: usize) -> Self {
        Self {
            key,
            offset,
            kind: PointKind::Text,
        }
    }

    pub fn element(key: NodeKey, offset: usize) -> Self {
        Self {
            key,
            offset,
            kind: PointKind::Element,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Selection {
    #[default]
    None,
    /// A text caret or range. `anchor == focus` means a single caret.
    Range { anchor: Point, focus: Point },
    /// A whole decorator or block is selected (e.g. clicking a file chip).
    Node(NodeKey),
}

impl Selection {
    pub fn caret(point: Point) -> Self {
        Self::Range {
            anchor: point,
            focus: point,
        }
    }

    pub fn is_collapsed(&self) -> bool {
        matches!(self, Self::Range { anchor, focus } if anchor == focus)
    }

    pub fn primary_key(&self) -> Option<NodeKey> {
        match self {
            Self::Range { focus, .. } => Some(focus.key),
            Self::Node(k) => Some(*k),
            _ => None,
        }
    }
}
