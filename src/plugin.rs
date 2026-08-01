//! Plugin hooks for editor events and appended transactions.

use crate::selection::Selection;
use crate::state::EditorState;
use crate::step::Transaction;

/// A keymap binding in canonical form such as `"Mod-b"` or `"Backspace"`.
#[derive(Clone, Debug)]
pub struct KeyBinding {
    pub keys: String,
    pub command: Command,
}

/// A command returns a transaction when it applies to the current state.
pub type Command = fn(&EditorState) -> Option<Transaction>;

/// Events produced by the editor view.
#[derive(Clone, Debug)]
pub enum EditorEvent {
    Undo,
    Redo,
    SelectionChange(Selection),
}

/// Extension point for key bindings and transaction processing.
pub trait Plugin {
    fn handle_event(&mut self, _state: &EditorState, _event: &EditorEvent) -> Option<Transaction> {
        None
    }

    fn append_transaction(
        &mut self,
        _tr: &Transaction,
        _old_state: &EditorState,
        _new_state: &EditorState,
    ) -> Option<Transaction> {
        None
    }

    fn keymap(&self) -> Vec<KeyBinding> {
        Vec::new()
    }
}
