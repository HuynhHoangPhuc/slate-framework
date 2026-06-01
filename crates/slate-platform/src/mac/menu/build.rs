//! Recursive `PlatformMenu` → `NSMenu` construction.
//!
//! Every actionable item targets the shared [`MenuTarget`] responder via the
//! `slateMenuAction:` selector and carries its framework `MenuId` in the
//! `tag`. Menus set `autoenablesItems(false)` so enabled state is driven purely
//! by the model (the target responds to every selector, so AppKit's automatic
//! enabling would otherwise enable everything). The menu bar gets a synthesized
//! leading application menu (About / Quit) per macOS convention.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadOnly, msg_send, sel};
use objc2_app_kit::{NSEventModifierFlags, NSMenu, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSString};

use super::target::MenuTarget;
use crate::{Key, Modifiers, PlatformMenu, PlatformMenuItem};

/// Build the application menu bar: a leading app menu (About / Quit) followed
/// by the model's top-level items.
pub(crate) fn build_menu_bar(
    model: &PlatformMenu,
    target: &MenuTarget,
    mtm: MainThreadMarker,
) -> Retained<NSMenu> {
    let bar = new_menu(mtm);
    bar.addItem(&standard_app_menu(target, mtm));
    append_items(&bar, &model.items, target, mtm);
    bar
}

/// Build a standalone menu (context menu) from the model's items.
pub(crate) fn build_context_menu(
    model: &PlatformMenu,
    target: &MenuTarget,
    mtm: MainThreadMarker,
) -> Retained<NSMenu> {
    let menu = new_menu(mtm);
    append_items(&menu, &model.items, target, mtm);
    menu
}

fn new_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    // Model owns enabled state; disable AppKit's automatic per-responder
    // enabling (the target responds to every action selector).
    menu.setAutoenablesItems(false);
    menu
}

fn append_items(
    into: &NSMenu,
    items: &[PlatformMenuItem],
    target: &MenuTarget,
    mtm: MainThreadMarker,
) {
    for item in items {
        match item {
            PlatformMenuItem::Action {
                id,
                label,
                accelerator,
                enabled,
                checked,
            } => {
                let ns_item = make_action_item(*id, label, accelerator, *enabled, *checked, mtm);
                set_target(&ns_item, target);
                into.addItem(&ns_item);
            }
            PlatformMenuItem::Submenu {
                label,
                enabled,
                items,
            } => {
                let sub = new_menu(mtm);
                sub.setTitle(&NSString::from_str(label));
                append_items(&sub, items, target, mtm);
                let ns_item = NSMenuItem::new(mtm);
                ns_item.setTitle(&NSString::from_str(label));
                ns_item.setEnabled(*enabled);
                ns_item.setSubmenu(Some(&sub));
                into.addItem(&ns_item);
            }
            PlatformMenuItem::Separator => {
                into.addItem(&NSMenuItem::separatorItem(mtm));
            }
        }
    }
}

fn make_action_item(
    id: u64,
    label: &str,
    accelerator: &Option<(Modifiers, Key)>,
    enabled: bool,
    checked: bool,
    mtm: MainThreadMarker,
) -> Retained<NSMenuItem> {
    let key_equiv = accelerator
        .as_ref()
        .map(|(_, key)| key_equivalent_string(key))
        .unwrap_or_default();
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(label),
            Some(sel!(slateMenuAction:)),
            &NSString::from_str(&key_equiv),
        )
    };
    if let Some((mods, _)) = accelerator {
        item.setKeyEquivalentModifierMask(modifier_mask(*mods));
    }
    // `tag` carries the framework MenuId across to `slateMenuAction:`. NSInteger
    // is i64 on 64-bit macOS; realistic ids fit comfortably.
    item.setTag(id as isize);
    item.setEnabled(enabled);
    // setState: NSControlStateValueOn = 1, Off = 0. Sent raw to avoid pulling in
    // the NSCell feature solely for the NSControlStateValue typedef.
    // SAFETY: `setState:` takes an NSInteger; 0/1 are valid control states.
    let _: () = unsafe { msg_send![&item, setState: if checked { 1isize } else { 0isize }] };
    item
}

/// Wire an item's action target to the shared responder.
fn set_target(item: &NSMenuItem, target: &MenuTarget) {
    // Deref-coerce `&MenuTarget` to `&AnyObject` for the `setTarget:` arg.
    let target_ref: &AnyObject = target;
    // SAFETY: `target_ref` is a live, retained responder that outlives every
    // menu item (held in the module's thread-local `Retained<MenuTarget>`).
    unsafe { item.setTarget(Some(target_ref)) };
}

/// The standard leading macOS application menu: About + separator + Quit (⌘Q).
fn standard_app_menu(target: &MenuTarget, mtm: MainThreadMarker) -> Retained<NSMenuItem> {
    let app_menu = new_menu(mtm);

    // About → AppKit's standard about panel (first-responder chain, target nil).
    let about = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("About"),
            Some(sel!(orderFrontStandardAboutPanel:)),
            &NSString::from_str(""),
        )
    };
    app_menu.addItem(&about);

    app_menu.addItem(&NSMenuItem::separatorItem(mtm));

    // Quit → our responder's clean-shutdown path (not terminate:), ⌘Q.
    let quit = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Quit"),
            Some(sel!(slateQuit:)),
            &NSString::from_str("q"),
        )
    };
    quit.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
    set_target(&quit, target);
    app_menu.addItem(&quit);

    let app_item = NSMenuItem::new(mtm);
    app_item.setSubmenu(Some(&app_menu));
    app_item
}

/// Map a logical [`Key`] to an `NSMenuItem` key-equivalent string.
///
/// `Key::Character` lowercases (Shift is expressed via the modifier mask, not
/// case). Named / unidentified keys have no textual equivalent and produce an
/// empty string (no key equivalent) — extend here if a real need appears.
pub(crate) fn key_equivalent_string(key: &Key) -> String {
    match key {
        Key::Character(s) => s.to_lowercase(),
        Key::Named(_) | Key::Unidentified => String::new(),
    }
}

/// Map neutral [`Modifiers`] to an `NSEventModifierFlags` key-equivalent mask.
pub(crate) fn modifier_mask(mods: Modifiers) -> NSEventModifierFlags {
    let mut mask = NSEventModifierFlags::empty();
    if mods.meta {
        mask |= NSEventModifierFlags::Command;
    }
    if mods.ctrl {
        mask |= NSEventModifierFlags::Control;
    }
    if mods.alt {
        mask |= NSEventModifierFlags::Option;
    }
    if mods.shift {
        mask |= NSEventModifierFlags::Shift;
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_key_equivalent_lowercases() {
        assert_eq!(key_equivalent_string(&Key::Character("S".into())), "s");
        assert_eq!(key_equivalent_string(&Key::Character("n".into())), "n");
    }

    #[test]
    fn named_and_unidentified_have_no_key_equivalent() {
        assert!(key_equivalent_string(&Key::Unidentified).is_empty());
    }

    #[test]
    fn modifier_mask_maps_each_flag() {
        let cmd_shift = modifier_mask(Modifiers {
            meta: true,
            shift: true,
            ..Default::default()
        });
        assert!(cmd_shift.contains(NSEventModifierFlags::Command));
        assert!(cmd_shift.contains(NSEventModifierFlags::Shift));
        assert!(!cmd_shift.contains(NSEventModifierFlags::Control));
        assert!(!cmd_shift.contains(NSEventModifierFlags::Option));
    }

    #[test]
    fn empty_modifiers_map_to_empty_mask() {
        assert_eq!(modifier_mask(Modifiers::default()), NSEventModifierFlags::empty());
    }
}
