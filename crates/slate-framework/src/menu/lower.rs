//! Lowering the framework [`Menu`] model into the platform-neutral
//! [`PlatformMenu`] descriptor that native backends (P6b/P6c) consume.
//!
//! Kept separate from [`model`](super::model) so the ergonomic model and the
//! boundary translation evolve independently; closures never cross — only the
//! numeric [`MenuId`](super::model::MenuId) does.

use slate_platform::{PlatformMenu, PlatformMenuItem};

use super::model::{Menu, MenuItem};

impl Menu {
    /// Lower this model into the platform-neutral [`PlatformMenu`] descriptor.
    /// Recurses through submenus, copying ids/labels/accelerators/flags.
    pub fn to_platform(&self) -> PlatformMenu {
        PlatformMenu::new(self.items().iter().map(item_to_platform).collect())
    }
}

/// Lower a single [`MenuItem`] into its platform descriptor entry.
fn item_to_platform(item: &MenuItem) -> PlatformMenuItem {
    match item {
        MenuItem::Action(a) => PlatformMenuItem::Action {
            id: a.id.0,
            label: a.label.clone(),
            accelerator: a
                .accelerator
                .as_ref()
                .map(|acc| (acc.modifiers, acc.key.clone())),
            enabled: a.enabled,
            checked: a.checked,
        },
        MenuItem::Submenu(s) => PlatformMenuItem::Submenu {
            label: s.label.clone(),
            enabled: s.enabled,
            items: s.menu.items().iter().map(item_to_platform).collect(),
        },
        MenuItem::Separator => PlatformMenuItem::Separator,
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::sample_menu;
    use slate_platform::{Key, PlatformMenuItem};

    #[test]
    fn lowering_preserves_structure_ids_and_flags() {
        let platform = sample_menu().to_platform();
        assert_eq!(platform.items.len(), 2);

        let PlatformMenuItem::Submenu { label, items, .. } = &platform.items[0] else {
            panic!("expected submenu");
        };
        assert_eq!(label, "File");
        assert_eq!(items.len(), 3);

        // Open: id 1, enabled, Cmd+O accelerator.
        let PlatformMenuItem::Action {
            id, accelerator, ..
        } = &items[0]
        else {
            panic!("expected action");
        };
        assert_eq!(*id, 1);
        assert!(matches!(
            &items[0],
            PlatformMenuItem::Action { enabled: true, .. }
        ));
        let (mods, key) = accelerator.as_ref().expect("accelerator lowered");
        assert!(mods.meta);
        assert_eq!(*key, Key::Character("o".into()));

        // Separator preserved between Open and Quit.
        assert!(matches!(items[1], PlatformMenuItem::Separator));
        // Quit lowered as disabled.
        assert!(matches!(
            &items[2],
            PlatformMenuItem::Action { enabled: false, .. }
        ));
        // Top-level Toggle lowered with checked state.
        assert!(matches!(
            &platform.items[1],
            PlatformMenuItem::Action { checked: true, .. }
        ));
    }
}
