//! Editor runtime — state container, handle, dispatch pipeline.
//!
//! `EditorState` is the pure value type — `Doc` + `Selection` + a small bit
//! of UI state (pending mark format for the next keystroke). It clones
//! cheaply.
//!
//! `EditorHandle` is the live runtime: a Dioxus `Signal<EditorState>` plus
//! the plugin list (kept behind an `Rc<RefCell<…>>` because plugins are
//! not `Clone`). Host code obtains a handle via [`use_editor`] and uses it
//! to dispatch transactions, query state, run named commands, and read the
//! current document for serialization. The handle is what the
//! [`crate::view::EditorView`] component consumes.

use std::cell::RefCell;
use std::rc::Rc;

use dioxus::prelude::*;

use crate::format::FormatBits;
use crate::model::Doc;
use crate::plugin::{EditorEvent, KeyBinding, Plugin};
use crate::schema::Schema;
use crate::selection::{Point, PointKind, Selection};
use crate::step::{StepError, Transaction};

#[derive(Clone)]
pub struct EditorState {
    pub doc: Doc,
    pub selection: Selection,
    /// Override format for the next typed text. `Some(bits)` means typing
    /// will produce a fresh text node with those bits regardless of
    /// surrounding context (when bold was just toggled, subsequent
    /// characters should be bold even though the caret sits in a
    /// plain run"). `None` means typing inherits whatever the caret's
    /// surroundings already carry.
    pub pending_format: Option<FormatBits>,
    pub schema: Rc<Schema>,
}

impl EditorState {
    pub fn new(schema: Rc<Schema>, doc: Doc) -> Self {
        Self {
            doc,
            selection: Selection::None,
            pending_format: None,
            schema,
        }
    }

    pub fn apply(&self, tr: Transaction) -> Result<EditorState, StepError> {
        let pending_override = tr.pending_format;
        let (doc, sel) = tr.apply(self.doc.clone())?;
        Ok(EditorState {
            doc,
            selection: sel.unwrap_or_else(|| self.selection.clone()),
            // `pending_format(None)` on the tx is a no-op (carry the
            // prior value). To explicitly clear pending, a command sets
            // a fresh `Some(FormatBits::NONE)`, which is consumed by
            // the next insert_text and cleared by the auto-reset step.
            pending_format: pending_override.or(self.pending_format),
            schema: self.schema.clone(),
        })
    }
}

/// Failure while applying a transaction through the plugin pipeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchError {
    Step(StepError),
    Plugin(StepError),
    ChainExhausted,
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Step(error) => write!(f, "transaction failed: {error}"),
            Self::Plugin(error) => write!(f, "plugin transaction failed: {error}"),
            Self::ChainExhausted => write!(f, "plugin transaction chain exceeded 8 iterations"),
        }
    }
}

impl std::error::Error for DispatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Step(error) | Self::Plugin(error) => Some(error),
            Self::ChainExhausted => None,
        }
    }
}

pub struct EditorConfig {
    pub schema: Schema,
    pub plugins: Vec<Box<dyn Plugin>>,
    pub initial_doc: Doc,
}

impl EditorConfig {
    pub fn new(schema: Schema) -> Self {
        Self {
            schema,
            plugins: Vec::new(),
            initial_doc: Doc::empty(),
        }
    }

    pub fn with_plugin(mut self, plugin: Box<dyn Plugin>) -> Self {
        self.plugins.push(plugin);
        self
    }

    pub fn with_doc(mut self, doc: Doc) -> Self {
        self.initial_doc = doc;
        self
    }
}

/// Live editor instance — clone-cheap handle wrapping shared state and the
/// plugin pipeline. Components in the host application keep a copy and
/// dispatch transactions through it.
#[derive(Clone)]
pub struct EditorHandle {
    state: Signal<EditorState>,
    plugins: Rc<RefCell<Vec<Box<dyn Plugin>>>>,
    keymap: Rc<Vec<KeyBinding>>,
    error_handler: Rc<RefCell<Option<EventHandler<DispatchError>>>>,
    /// Weak to avoid a listener → handle → binding cycle retaining editor
    /// state after the component unmounts. The hook bundle owns the binding.
    #[cfg(target_arch = "wasm32")]
    dom: std::rc::Weak<RefCell<crate::view::DomBinding>>,
}

impl PartialEq for EditorHandle {
    fn eq(&self, other: &Self) -> bool {
        // Identity: two handles are equal when they share the same plugin
        // pipeline. The signal value can change every dispatch, so its
        // contents aren't a meaningful comparison; the plugin `Rc` is the
        // stable identity anchor.
        Rc::ptr_eq(&self.plugins, &other.plugins)
    }
}

impl EditorHandle {
    pub fn state_signal(&self) -> Signal<EditorState> {
        self.state
    }

    pub fn read_state(&self) -> EditorState {
        self.state.read().clone()
    }

    pub fn doc(&self) -> Doc {
        self.state.read().doc.clone()
    }

    pub fn selection(&self) -> Selection {
        self.state.read().selection.clone()
    }

    /// Apply a transaction. Drives the full pipeline:
    /// 1. Compute the new state.
    /// 2. Run plugin `append_transaction` hooks against (prev → next); chain
    ///    any returned transactions until the pipeline settles (capped to
    ///    avoid loops).
    /// 3. Commit to the signal.
    ///
    /// `prev` is the state immediately before the most-recently-applied
    /// transaction (not the very first state in the dispatch cycle) so
    /// plugins like history see exactly the (before, after) pair that
    /// corresponds to one transaction.
    pub fn dispatch(&self, tr: Transaction) -> Result<(), DispatchError> {
        if tr.is_empty() {
            return Ok(());
        }
        let mut state_sig = self.state;
        let mut prev = state_sig.read().clone();
        let mut next = prev.apply(tr.clone()).map_err(DispatchError::Step)?;
        let mut latest_tr = tr;
        let mut iterations = 0;
        loop {
            let chained = {
                let mut plugins = self.plugins.borrow_mut();
                plugins
                    .iter_mut()
                    .find_map(|plugin| plugin.append_transaction(&latest_tr, &prev, &next))
            };
            let Some(chained) = chained.filter(|transaction| !transaction.is_empty()) else {
                break;
            };
            if iterations >= 8 {
                return Err(DispatchError::ChainExhausted);
            }
            iterations += 1;
            prev = next.clone();
            next = prev.apply(chained.clone()).map_err(DispatchError::Plugin)?;
            latest_tr = chained;
        }
        state_sig.set(next);
        Ok(())
    }

    /// Send an event through plugins. First plugin to return a transaction
    /// "consumes" the event; the transaction is dispatched and `true` is
    /// returned. `false` means no plugin handled it.
    pub fn handle_event(&self, event: EditorEvent) -> Result<bool, DispatchError> {
        let tr_opt: Option<Transaction> = {
            let mut plugins = self.plugins.borrow_mut();
            let state = self.state.read().clone();
            let mut hit = None;
            for p in plugins.iter_mut() {
                if let Some(t) = p.handle_event(&state, &event) {
                    hit = Some(t);
                    break;
                }
            }
            hit
        };
        if let Some(tr) = tr_opt {
            self.dispatch(tr)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Look up a command bound to `keys` (canonical form like `"Mod-b"`)
    /// and run it. Returns `true` when a binding fired.
    pub fn run_key(&self, keys: &str) -> Result<bool, DispatchError> {
        let cmd = self
            .keymap
            .iter()
            .find(|b| b.keys.eq_ignore_ascii_case(keys))
            .map(|b| b.command);
        let Some(cmd) = cmd else {
            return Ok(false);
        };
        let state = self.state.read().clone();
        if let Some(tr) = cmd(&state) {
            self.dispatch(tr)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn set_selection(&self, sel: Selection) {
        let mut state_sig = self.state;
        let cur = state_sig.read().selection.clone();
        if cur == sel {
            return;
        }
        let mut next = state_sig.read().clone();
        next.selection = sel;
        state_sig.set(next);
    }

    /// Replace the document wholesale (e.g. after `from_markdown`). Resets
    /// selection to "caret inside the first block" — anchoring at the doc
    /// root would let the next typed character land as a root-level text
    /// sibling instead of inside the paragraph.
    pub fn set_doc(&self, doc: Doc) {
        let mut state_sig = self.state;
        let mut next = state_sig.read().clone();
        let first_block = doc
            .root_node()
            .children
            .first()
            .copied()
            .unwrap_or(doc.root_key());
        next.doc = doc;
        next.selection = Selection::caret(Point {
            key: first_block,
            offset: 0,
            kind: PointKind::Element,
        });
        next.pending_format = None;
        state_sig.set(next);
    }

    pub(crate) fn set_error_handler(&self, handler: Option<EventHandler<DispatchError>>) {
        *self.error_handler.borrow_mut() = handler;
    }

    /// Report failures from UI-owned dispatch paths. Public `dispatch`
    /// remains fallible for host code that wants to handle errors directly.
    pub(crate) fn report_internal<T>(&self, result: Result<T, DispatchError>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                if let Some(handler) = *self.error_handler.borrow() {
                    handler.call(error);
                } else {
                    #[cfg(target_arch = "wasm32")]
                    web_sys::console::error_1(&format!("dioxus-editor: {error}").into());
                    #[cfg(not(target_arch = "wasm32"))]
                    eprintln!("dioxus-editor: {error}");
                }
                None
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn dom_binding(&self) -> Rc<RefCell<crate::view::DomBinding>> {
        self.dom
            .upgrade()
            .expect("editor DOM binding outlived its component")
    }
}

#[derive(Clone)]
struct EditorBundle {
    seed: Rc<RefCell<Option<EditorState>>>,
    plugins: Rc<RefCell<Vec<Box<dyn Plugin>>>>,
    keymap: Rc<Vec<KeyBinding>>,
    error_handler: Rc<RefCell<Option<EventHandler<DispatchError>>>>,
    #[cfg(target_arch = "wasm32")]
    dom: Rc<RefCell<crate::view::DomBinding>>,
}

/// Dioxus hook that creates a fresh editor instance. Call once per editor;
/// the returned handle is a `Clone`-cheap value that may be passed to
/// children freely.
pub fn use_editor(make_config: impl FnOnce() -> EditorConfig) -> EditorHandle {
    // First render: evaluate the config, set up the bundle. Subsequent
    // renders get the same bundle back from the hook slot.
    let bundle = use_hook(move || {
        let cfg = make_config();
        let schema = Rc::new(cfg.schema);
        let mut keymap = Vec::new();
        for p in &cfg.plugins {
            keymap.extend(p.keymap());
        }
        EditorBundle {
            seed: Rc::new(RefCell::new(Some(EditorState::new(
                schema,
                cfg.initial_doc,
            )))),
            plugins: Rc::new(RefCell::new(cfg.plugins)),
            keymap: Rc::new(keymap),
            error_handler: Rc::new(RefCell::new(None)),
            #[cfg(target_arch = "wasm32")]
            dom: Rc::new(RefCell::new(crate::view::DomBinding::default())),
        }
    });
    let seed = bundle.seed.clone();
    let state = use_signal(move || {
        seed.borrow_mut()
            .take()
            .expect("EditorState seed must be present on first render")
    });
    EditorHandle {
        state,
        plugins: bundle.plugins,
        keymap: bundle.keymap,
        error_handler: bundle.error_handler,
        #[cfg(target_arch = "wasm32")]
        dom: Rc::downgrade(&bundle.dom),
    }
}
