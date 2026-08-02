//! A pluggable rich-text editor for Dioxus 0.7.
//!
//! Mutations are expressed as [`Transaction`]s, application-defined rich
//! content is registered with [`DecoratorSpec`], and [`EditorView`] renders
//! the document to an accessible contenteditable surface.
//!
//! ```no_run
//! use dioxus::prelude::*;
//! use dioxus_editor::{plugins, use_editor, EditorConfig, EditorView, Schema};
//!
//! #[component]
//! fn Editor() -> Element {
//!     let editor = use_editor(|| {
//!         EditorConfig::new(Schema::new())
//!             .with_plugin(plugins::DefaultKeymap)
//!             .with_plugin(plugins::History::new())
//!     });
//!
//!     rsx! { EditorView { editor } }
//! }
//! ```

mod attrs;
mod autolink;
pub mod commands;
mod format;
pub mod io;
mod model;
mod plugin;
pub mod plugins;
mod schema;
mod selection;
mod state;
mod step;
mod view;

pub use attrs::{AttrValue, Attrs};
pub use format::FormatBits;
pub use model::{DecoratorNode, Doc, ElementNode, Node, NodeKey, TextNode};
pub use plugin::{Command, EditorEvent, KeyBinding, Plugin};
pub use schema::{DecoratorRenderer, DecoratorSpec, Schema};
pub use selection::{Point, PointKind, Selection};
pub use state::{DispatchError, EditorConfig, EditorHandle, EditorState, use_editor};
pub use step::{NodeSpec, Step, StepError, Transaction};
pub use view::EditorView;
