//! The edit menu a touch screen shows over a text field's selection (Cut,
//! Copy, Paste, Select All): what the App asks a backend to show, and what
//! the user picked. iOS shows it with `UIEditMenuInteraction`, Android as a
//! floating `ActionMode`; elsewhere (the web, older systems) the App draws
//! its own. See `docs/text-editing.md`.

use crate::geometry::Rect;

/// An editing command from the edit menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EditAction {
    /// Copy the selection to the clipboard and delete it.
    Cut,
    /// Copy the selection to the clipboard.
    Copy,
    /// Insert the clipboard's text at the caret (over the selection).
    Paste,
    /// Select the field's whole text.
    SelectAll,
}

impl EditAction {
    /// Every action, in the order the menu lists them.
    pub const ALL: [EditAction; 4] = [
        EditAction::Cut,
        EditAction::Copy,
        EditAction::Paste,
        EditAction::SelectAll,
    ];

    /// The item's label, in English (the system menus use their own).
    pub fn label(self) -> &'static str {
        match self {
            EditAction::Cut => "Cut",
            EditAction::Copy => "Copy",
            EditAction::Paste => "Paste",
            EditAction::SelectAll => "Select All",
        }
    }

    /// A small number standing for the action (a menu item's id on Android,
    /// a log line in tests): 1 to 4, in [`ALL`](Self::ALL)'s order.
    pub fn id(self) -> i32 {
        match self {
            EditAction::Cut => 1,
            EditAction::Copy => 2,
            EditAction::Paste => 3,
            EditAction::SelectAll => 4,
        }
    }

    /// The action with this [`id`](Self::id).
    pub fn from_id(id: i32) -> Option<EditAction> {
        EditAction::ALL.into_iter().find(|a| a.id() == id)
    }
}

/// The edit menu to show: where (the selection, or the caret, in logical
/// pixels of the window's content; the menu goes above it, or below when
/// there's no room) and which actions it offers.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct EditMenu {
    /// What the menu points at.
    pub rect: Rect,
    pub cut: bool,
    pub copy: bool,
    pub paste: bool,
    pub select_all: bool,
}

impl EditMenu {
    /// Whether the menu offers `action`.
    pub fn offers(&self, action: EditAction) -> bool {
        match action {
            EditAction::Cut => self.cut,
            EditAction::Copy => self.copy,
            EditAction::Paste => self.paste,
            EditAction::SelectAll => self.select_all,
        }
    }

    /// The actions offered, in menu order.
    pub fn actions(&self) -> Vec<EditAction> {
        EditAction::ALL
            .into_iter()
            .filter(|a| self.offers(*a))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_actions_follow_the_flags() {
        for a in EditAction::ALL {
            assert_eq!(EditAction::from_id(a.id()), Some(a));
        }
        assert_eq!(EditAction::from_id(0), None);
        let menu = EditMenu {
            rect: Rect::ZERO,
            cut: false,
            copy: true,
            paste: true,
            select_all: false,
        };
        assert_eq!(menu.actions(), [EditAction::Copy, EditAction::Paste]);
    }
}
