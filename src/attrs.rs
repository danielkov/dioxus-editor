//! Typed attribute bags for editor nodes.
//!
//! Decorator nodes (mentions, file chips, link previews) and elements carry
//! arbitrary key/value payloads. `Attrs` is a small, ordered map so iteration
//! is stable for serialization; `AttrValue` is a closed sum of the primitive
//! types we round-trip to markdown / JSON.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Attrs(BTreeMap<String, AttrValue>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttrValue {
    Str(String),
    Int(i64),
    Bool(bool),
}

impl AttrValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            AttrValue::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            AttrValue::Int(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            AttrValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

impl Attrs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, key: impl Into<String>, value: impl Into<AttrValue>) -> Self {
        self.insert(key, value);
        self
    }

    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<AttrValue>) {
        self.0.insert(key.into(), value.into());
    }

    pub fn remove(&mut self, key: &str) -> Option<AttrValue> {
        self.0.remove(key)
    }

    pub fn get(&self, key: &str) -> Option<&AttrValue> {
        self.0.get(key)
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(AttrValue::as_str)
    }

    pub fn get_int(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(AttrValue::as_int)
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(AttrValue::as_bool)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &AttrValue)> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl From<&str> for AttrValue {
    fn from(s: &str) -> Self {
        AttrValue::Str(s.to_string())
    }
}
impl From<String> for AttrValue {
    fn from(s: String) -> Self {
        AttrValue::Str(s)
    }
}
impl From<i64> for AttrValue {
    fn from(n: i64) -> Self {
        AttrValue::Int(n)
    }
}
impl From<bool> for AttrValue {
    fn from(b: bool) -> Self {
        AttrValue::Bool(b)
    }
}
