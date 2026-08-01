//! Inline text formats expressed as a bitmask.
//!
//! Bold / italic / strike / code overlap on the same character ranges and
//! toggle independently, so a bitfield is the most natural representation.
//! Each format maps to a CSS class on the rendered span and to a markdown
//! marker pair on serialization.

use std::ops::{BitAnd, BitOr, BitOrAssign, Not};

#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Debug)]
pub struct FormatBits(pub u8);

impl FormatBits {
    pub const NONE: Self = Self(0);
    pub const BOLD: Self = Self(1 << 0);
    pub const ITALIC: Self = Self(1 << 1);
    pub const STRIKE: Self = Self(1 << 2);
    pub const CODE: Self = Self(1 << 3);

    pub const ALL: &'static [Self] = &[Self::BOLD, Self::ITALIC, Self::STRIKE, Self::CODE];

    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }

    pub fn toggle(&mut self, other: Self) {
        self.0 ^= other.0;
    }

    pub fn css_class_suffix(self) -> String {
        let mut s = String::new();
        if self.contains(Self::BOLD) {
            s.push_str(" e-b");
        }
        if self.contains(Self::ITALIC) {
            s.push_str(" e-i");
        }
        if self.contains(Self::STRIKE) {
            s.push_str(" e-s");
        }
        if self.contains(Self::CODE) {
            s.push_str(" e-c");
        }
        s
    }
}

impl BitOr for FormatBits {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for FormatBits {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for FormatBits {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl Not for FormatBits {
    type Output = Self;
    fn not(self) -> Self {
        Self(!self.0)
    }
}
