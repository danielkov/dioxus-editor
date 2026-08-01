//! Decorator registry for application-defined rich content.

use std::collections::HashMap;
use std::rc::Rc;

use dioxus::prelude::*;

use crate::attrs::Attrs;

/// Renderer for a decorator node.
pub type DecoratorRenderer = Rc<dyn Fn(&Attrs) -> Element>;

/// Defines rendering and markdown serialization for a decorator kind.
#[derive(Clone)]
pub struct DecoratorSpec {
    /// Whether the decorator flows inline with text.
    pub inline: bool,
    /// Render the decorator's application-defined contents.
    ///
    /// Attributes may originate in untrusted markdown or application data.
    /// Renderers must validate URLs and other security-sensitive attributes
    /// before placing them in the DOM.
    pub render: DecoratorRenderer,
    /// Serialize the decorator to markdown.
    pub to_markdown: Rc<dyn Fn(&Attrs) -> String>,
}

/// Immutable-after-construction registry of decorator kinds.
#[derive(Clone, Default)]
pub struct Schema {
    decorators: HashMap<String, DecoratorSpec>,
}

impl Schema {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_decorator(mut self, name: impl Into<String>, spec: DecoratorSpec) -> Self {
        self.decorators.insert(name.into(), spec);
        self
    }

    pub fn decorator(&self, kind: &str) -> Option<&DecoratorSpec> {
        self.decorators.get(kind)
    }

    pub fn has_decorator(&self, kind: &str) -> bool {
        self.decorators.contains_key(kind)
    }

    pub fn decorator_kinds(&self) -> impl Iterator<Item = &str> {
        self.decorators.keys().map(String::as_str)
    }
}
