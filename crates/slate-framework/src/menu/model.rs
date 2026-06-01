//! Platform-agnostic menu model.
//!
//! A [`Menu`] is a `Clone`, closure-free tree of [`MenuItem`]s describing a
//! menu bar or context menu. Action closures are NOT stored here — each
//! actionable item carries a stable [`MenuId`], and handlers are registered
//! separately in the [`routing`](super::routing) registry keyed by that id.
//! This keeps the model cheap to clone and rebuild reactively, and lets the
//! same model lower to a native `PlatformMenu` (closures stay framework-side).
//! Lowering lives in [`lower`](super::lower) (`Menu::to_platform`).

use slate_platform::{Key, Modifiers};

/// Stable identifier for an actionable [`MenuItem`].
///
/// Adopters mint their own ids (e.g. one per command) and register a handler
/// for each via the action registry. The numeric value crosses the
/// framework→platform boundary so native backends can route activation back.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct MenuId(pub u64);

/// Neutral keyboard accelerator (key equivalent) for a menu command.
///
/// Stores a modifier set plus a logical [`Key`]; backends map it to an
/// `NSMenuItem` key-equivalent (macOS) or a Win32 accelerator (Windows).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accelerator {
    /// Modifier keys that must be held (Cmd/Ctrl/Alt/Shift).
    pub modifiers: Modifiers,
    /// Logical key (e.g. `Key::Character("s")` for Cmd+S).
    pub key: Key,
}

impl Accelerator {
    /// Build an accelerator from modifiers and a logical key.
    pub fn new(modifiers: Modifiers, key: Key) -> Self {
        Self { modifiers, key }
    }
}

/// An actionable menu command.
#[derive(Clone, Debug)]
pub struct MenuAction {
    /// Routing id; activation dispatches to the handler registered for it.
    pub id: MenuId,
    /// Display + accessible label.
    pub label: String,
    /// Optional keyboard accelerator.
    pub accelerator: Option<Accelerator>,
    /// Greyed-out and non-activatable when `false`.
    pub enabled: bool,
    /// Shows a check mark when `true` (toggle commands).
    pub checked: bool,
}

impl MenuAction {
    /// Create an enabled, unchecked action with no accelerator.
    pub fn new(id: MenuId, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            accelerator: None,
            enabled: true,
            checked: false,
        }
    }

    /// Attach a keyboard accelerator.
    pub fn accelerator(mut self, accel: Accelerator) -> Self {
        self.accelerator = Some(accel);
        self
    }

    /// Set the enabled state (default `true`).
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set the checked state (default `false`).
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
}

/// A nested submenu with its own title and child items.
#[derive(Clone, Debug)]
pub struct SubMenu {
    /// Submenu title.
    pub label: String,
    /// Child menu.
    pub menu: Menu,
    /// Greyed-out when `false`.
    pub enabled: bool,
}

/// One entry in a [`Menu`]: a command, a submenu, or a separator.
#[derive(Clone, Debug)]
pub enum MenuItem {
    /// Actionable command.
    Action(MenuAction),
    /// Nested submenu.
    Submenu(SubMenu),
    /// Horizontal separator line.
    Separator,
}

/// A platform-agnostic menu: an ordered list of [`MenuItem`]s.
///
/// Build with the chaining helpers, then hand to
/// [`AppContext::set_menu`](crate::AppContext::set_menu). Cheap to clone and
/// rebuild on each render so enabled/checked state can track signals.
#[derive(Clone, Debug, Default)]
pub struct Menu {
    items: Vec<MenuItem>,
}

impl Menu {
    /// Create an empty menu.
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Append an actionable command.
    pub fn action(mut self, action: MenuAction) -> Self {
        self.items.push(MenuItem::Action(action));
        self
    }

    /// Append a submenu.
    pub fn submenu(mut self, label: impl Into<String>, menu: Menu) -> Self {
        self.items.push(MenuItem::Submenu(SubMenu {
            label: label.into(),
            menu,
            enabled: true,
        }));
        self
    }

    /// Append a separator line.
    pub fn separator(mut self) -> Self {
        self.items.push(MenuItem::Separator);
        self
    }

    /// The top-level items, in order.
    pub fn items(&self) -> &[MenuItem] {
        &self.items
    }

    /// Find the [`MenuAction`] carrying `id`, searching submenus recursively.
    pub fn find_action(&self, id: MenuId) -> Option<&MenuAction> {
        for item in &self.items {
            match item {
                MenuItem::Action(a) if a.id == id => return Some(a),
                MenuItem::Submenu(s) => {
                    if let Some(found) = s.menu.find_action(id) {
                        return Some(found);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Whether the action carrying `id` is enabled. Returns `true` for ids not
    /// present in this model (the registry may outlive a particular menu), so
    /// callers treat "unknown" as activatable and let routing decide.
    pub fn is_enabled(&self, id: MenuId) -> bool {
        self.find_action(id).map(|a| a.enabled).unwrap_or(true)
    }
}

#[cfg(test)]
pub(super) fn sample_menu() -> Menu {
    // File > [Open (Cmd+O), ---, Quit (disabled)]  + Toggle (checked)
    let file = Menu::new()
        .action(
            MenuAction::new(MenuId(1), "Open").accelerator(Accelerator::new(
                Modifiers {
                    meta: true,
                    ..Default::default()
                },
                Key::Character("o".into()),
            )),
        )
        .separator()
        .action(MenuAction::new(MenuId(2), "Quit").enabled(false));
    Menu::new()
        .submenu("File", file)
        .action(MenuAction::new(MenuId(3), "Toggle").checked(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_nested_items_with_separators() {
        let menu = sample_menu();
        assert_eq!(menu.items().len(), 2);
        assert!(matches!(menu.items()[0], MenuItem::Submenu(_)));
        let MenuItem::Submenu(file) = &menu.items()[0] else {
            unreachable!()
        };
        assert_eq!(file.label, "File");
        assert_eq!(file.menu.items().len(), 3);
        assert!(matches!(file.menu.items()[1], MenuItem::Separator));
    }

    #[test]
    fn find_action_recurses_submenus() {
        let menu = sample_menu();
        assert_eq!(menu.find_action(MenuId(1)).unwrap().label, "Open");
        assert_eq!(menu.find_action(MenuId(2)).unwrap().label, "Quit");
        assert_eq!(menu.find_action(MenuId(3)).unwrap().label, "Toggle");
        assert!(menu.find_action(MenuId(99)).is_none());
    }

    #[test]
    fn is_enabled_reflects_flag_and_defaults_true_for_unknown() {
        let menu = sample_menu();
        assert!(menu.is_enabled(MenuId(1))); // explicit default-enabled
        assert!(!menu.is_enabled(MenuId(2))); // .enabled(false)
        assert!(menu.is_enabled(MenuId(99))); // unknown -> true
    }
}
