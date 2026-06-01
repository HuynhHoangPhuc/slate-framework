//! Keyboard, mouse, modifier-flag, and tracking-area handling for [`MetalView`].
//!
//! The `#[unsafe(method(...))]` selector declarations themselves live in the
//! parent [`super`] module's `define_class!` block (objc2 requires the
//! protocol/method declarations to be inline). Each selector body in the
//! macro is a one-line delegation into one of the `responder_*` methods
//! defined below.
//!
//! All public methods are invoked from inside [`super::super::ffi_boundary`]
//! at the macro call sites, so panics cannot cross the Obj-C ABI.

use objc2::{AnyThread, DefinedClass, msg_send};
use objc2_app_kit::{
    NSEvent, NSEventModifierFlags, NSTrackingArea, NSTrackingAreaOptions, NSWindow,
};
use objc2_foundation::{MainThreadMarker, NSArray, NSNotification};

use super::super::{dispatch_event, ffi_boundary, keymap};
use super::{MetalView, PendingKey, WindowDelegate};
use crate::{Event, Key, KeyCode, Modifiers, MouseButton, NamedKey, WindowId};

// ---------------------------------------------------------------------------
// Mouse event decode helpers
// ---------------------------------------------------------------------------

/// Decode position from NSEvent, flipping Y to top-left origin.
/// Returns logical position in view coordinates (no scale division).
fn decode_position(view: &MetalView, event: &NSEvent) -> (f32, f32) {
    let loc_in_window = event.locationInWindow();
    let bounds = view.bounds();
    let bounds_height = bounds.size.height as f32;

    let view_pt = view.convertPoint_fromView(loc_in_window, None);
    // NSView returns logical (point) coordinates; just flip Y to top-left origin.
    let x = view_pt.x as f32;
    let y = bounds_height - view_pt.y as f32;
    (x, y)
}

/// Pure decode_position logic for unit testing.
/// Takes (x, y) in window coords (Y-up), bounds_height, and scale.
/// Scale is ignored because NSView already returns logical (point) coordinates.
#[cfg(test)]
pub(crate) fn decode_position_pure(
    loc_in_window: (f32, f32),
    bounds_height: f32,
    _scale: f32,
) -> (f32, f32) {
    let x = loc_in_window.0;
    let y = bounds_height - loc_in_window.1;
    (x, y)
}

/// Decode modifier flags from NSEvent.
fn decode_modifiers(flags: NSEventModifierFlags) -> Modifiers {
    Modifiers {
        shift: flags.contains(NSEventModifierFlags::Shift),
        ctrl: flags.contains(NSEventModifierFlags::Control),
        alt: flags.contains(NSEventModifierFlags::Option),
        meta: flags.contains(NSEventModifierFlags::Command),
    }
}

/// Decode button number to MouseButton. Returns None for unsupported buttons.
fn decode_button(button_number: isize) -> Option<MouseButton> {
    match button_number {
        0 => Some(MouseButton::Left),
        1 => Some(MouseButton::Right),
        2 => Some(MouseButton::Middle),
        3..=7 => Some(MouseButton::Other((button_number - 3) as u8)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Keyboard event decode helpers
// ---------------------------------------------------------------------------

/// Map a [`KeyCode`] to the matching [`NamedKey`] when the key has no
/// textual representation. Returns `None` for letters / digits / punctuation
/// (those flow through `Key::Character`).
fn named_key_for_code(code: KeyCode) -> Option<NamedKey> {
    Some(match code {
        KeyCode::Enter => NamedKey::Enter,
        KeyCode::Tab => NamedKey::Tab,
        KeyCode::Escape => NamedKey::Escape,
        KeyCode::Backspace => NamedKey::Backspace,
        KeyCode::Delete => NamedKey::Delete,
        KeyCode::Space => NamedKey::Space,
        KeyCode::ArrowUp => NamedKey::ArrowUp,
        KeyCode::ArrowDown => NamedKey::ArrowDown,
        KeyCode::ArrowLeft => NamedKey::ArrowLeft,
        KeyCode::ArrowRight => NamedKey::ArrowRight,
        KeyCode::Home => NamedKey::Home,
        KeyCode::End => NamedKey::End,
        KeyCode::PageUp => NamedKey::PageUp,
        KeyCode::PageDown => NamedKey::PageDown,
        KeyCode::F1 => NamedKey::F1,
        KeyCode::F2 => NamedKey::F2,
        KeyCode::F3 => NamedKey::F3,
        KeyCode::F4 => NamedKey::F4,
        KeyCode::F5 => NamedKey::F5,
        KeyCode::F6 => NamedKey::F6,
        KeyCode::F7 => NamedKey::F7,
        KeyCode::F8 => NamedKey::F8,
        KeyCode::F9 => NamedKey::F9,
        KeyCode::F10 => NamedKey::F10,
        KeyCode::F11 => NamedKey::F11,
        KeyCode::F12 => NamedKey::F12,
        KeyCode::ShiftLeft | KeyCode::ShiftRight => NamedKey::Shift,
        KeyCode::ControlLeft | KeyCode::ControlRight => NamedKey::Control,
        KeyCode::AltLeft | KeyCode::AltRight => NamedKey::Alt,
        KeyCode::MetaLeft | KeyCode::MetaRight => NamedKey::Meta,
        _ => return None,
    })
}

/// Decode the logical [`Key`] for a `keyDown:`/`keyUp:` event.
///
/// Prefers `NamedKey` for known non-textual keys; otherwise falls back to
/// `Key::Character` when AppKit produced text, or `Key::Unidentified`.
fn decode_key(code: KeyCode, chars: &str) -> Key {
    if let Some(named) = named_key_for_code(code) {
        return Key::Named(named);
    }
    if chars.is_empty() {
        Key::Unidentified
    } else {
        Key::Character(chars.to_string())
    }
}

/// Returns true if the string contains at least one non-control character.
/// Filters out arrow / F-key sequences AppKit places in `characters`.
#[allow(dead_code)]
fn is_visible_text(s: &str) -> bool {
    s.chars().any(|c| !c.is_control())
}

/// Diff `prev` vs `new` modifier flags and dispatch synthetic [`Event::KeyDown`]
/// / [`Event::KeyUp`] for each modifier that toggled.
fn emit_modifier_changes(window: WindowId, prev: NSEventModifierFlags, new: NSEventModifierFlags) {
    let entries: [(NSEventModifierFlags, KeyCode, NamedKey); 4] = [
        (
            NSEventModifierFlags::Shift,
            KeyCode::ShiftLeft,
            NamedKey::Shift,
        ),
        (
            NSEventModifierFlags::Control,
            KeyCode::ControlLeft,
            NamedKey::Control,
        ),
        (
            NSEventModifierFlags::Option,
            KeyCode::AltLeft,
            NamedKey::Alt,
        ),
        (
            NSEventModifierFlags::Command,
            KeyCode::MetaLeft,
            NamedKey::Meta,
        ),
    ];
    let modifiers = decode_modifiers(new);
    for (flag, code, named) in entries {
        let was = prev.contains(flag);
        let is = new.contains(flag);
        if was == is {
            continue;
        }
        if is {
            dispatch_event(Event::KeyDown {
                window,
                code,
                key: Key::Named(named),
                modifiers,
                is_repeat: false,
            });
        } else {
            dispatch_event(Event::KeyUp {
                window,
                code,
                key: Key::Named(named),
                modifiers,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// MetalView responder method bodies
// ---------------------------------------------------------------------------

impl MetalView {
    pub(super) fn responder_mouse_down(&self, event: &NSEvent) {
        let id = self.ivars().window_id.get();
        ffi_boundary(|| {
            let position = decode_position(self, event);
            let modifiers = decode_modifiers(event.modifierFlags());
            dispatch_event(Event::MouseDown {
                window: id,
                position,
                button: MouseButton::Left,
                modifiers,
            });
        });
    }

    pub(super) fn responder_mouse_up(&self, event: &NSEvent) {
        let id = self.ivars().window_id.get();
        ffi_boundary(|| {
            let position = decode_position(self, event);
            let modifiers = decode_modifiers(event.modifierFlags());
            dispatch_event(Event::MouseUp {
                window: id,
                position,
                button: MouseButton::Left,
                modifiers,
            });
        });
    }

    pub(super) fn responder_mouse_dragged(&self, event: &NSEvent) {
        let id = self.ivars().window_id.get();
        ffi_boundary(|| {
            let position = decode_position(self, event);
            let modifiers = decode_modifiers(event.modifierFlags());
            dispatch_event(Event::MouseMoved {
                window: id,
                position,
                modifiers,
            });
        });
    }

    pub(super) fn responder_right_mouse_down(&self, event: &NSEvent) {
        let id = self.ivars().window_id.get();
        ffi_boundary(|| {
            let position = decode_position(self, event);
            let modifiers = decode_modifiers(event.modifierFlags());
            dispatch_event(Event::MouseDown {
                window: id,
                position,
                button: MouseButton::Right,
                modifiers,
            });
        });
    }

    pub(super) fn responder_right_mouse_up(&self, event: &NSEvent) {
        let id = self.ivars().window_id.get();
        ffi_boundary(|| {
            let position = decode_position(self, event);
            let modifiers = decode_modifiers(event.modifierFlags());
            dispatch_event(Event::MouseUp {
                window: id,
                position,
                button: MouseButton::Right,
                modifiers,
            });
        });
    }

    pub(super) fn responder_right_mouse_dragged(&self, event: &NSEvent) {
        let id = self.ivars().window_id.get();
        ffi_boundary(|| {
            let position = decode_position(self, event);
            let modifiers = decode_modifiers(event.modifierFlags());
            dispatch_event(Event::MouseMoved {
                window: id,
                position,
                modifiers,
            });
        });
    }

    pub(super) fn responder_other_mouse_down(&self, event: &NSEvent) {
        let id = self.ivars().window_id.get();
        ffi_boundary(|| {
            let button_num = event.buttonNumber();
            let Some(button) = decode_button(button_num) else {
                return;
            };
            let position = decode_position(self, event);
            let modifiers = decode_modifiers(event.modifierFlags());
            dispatch_event(Event::MouseDown {
                window: id,
                position,
                button,
                modifiers,
            });
        });
    }

    pub(super) fn responder_other_mouse_up(&self, event: &NSEvent) {
        let id = self.ivars().window_id.get();
        ffi_boundary(|| {
            let button_num = event.buttonNumber();
            let Some(button) = decode_button(button_num) else {
                return;
            };
            let position = decode_position(self, event);
            let modifiers = decode_modifiers(event.modifierFlags());
            dispatch_event(Event::MouseUp {
                window: id,
                position,
                button,
                modifiers,
            });
        });
    }

    pub(super) fn responder_other_mouse_dragged(&self, event: &NSEvent) {
        let id = self.ivars().window_id.get();
        ffi_boundary(|| {
            let position = decode_position(self, event);
            let modifiers = decode_modifiers(event.modifierFlags());
            dispatch_event(Event::MouseMoved {
                window: id,
                position,
                modifiers,
            });
        });
    }

    pub(super) fn responder_mouse_moved(&self, event: &NSEvent) {
        let id = self.ivars().window_id.get();
        ffi_boundary(|| {
            let position = decode_position(self, event);
            let modifiers = decode_modifiers(event.modifierFlags());
            dispatch_event(Event::MouseMoved {
                window: id,
                position,
                modifiers,
            });
        });
    }

    pub(super) fn responder_mouse_exited(&self) {
        let id = self.ivars().window_id.get();
        ffi_boundary(|| {
            dispatch_event(Event::MouseExited { window: id });
        });
    }

    pub(super) fn responder_scroll_wheel(&self, event: &NSEvent) {
        let id = self.ivars().window_id.get();
        ffi_boundary(|| {
            let position = decode_position(self, event);
            let modifiers = decode_modifiers(event.modifierFlags());
            let precise = event.hasPreciseScrollingDeltas();
            let (delta_x, delta_y) = if precise {
                (
                    event.scrollingDeltaX() as f32,
                    event.scrollingDeltaY() as f32,
                )
            } else {
                (event.deltaX() as f32, event.deltaY() as f32)
            };
            dispatch_event(Event::MouseScrolled {
                window: id,
                position,
                delta_x,
                delta_y,
                precise,
                modifiers,
            });
        });
    }

    /// `keyDown:` body. See parent module's `define_class!` selector for the
    /// IME-first routing rationale.
    pub(super) fn responder_key_down(&self, event: &NSEvent) {
        let id = self.ivars().window_id.get();
        ffi_boundary(|| {
            let vk = event.keyCode();
            let code = keymap::decode_keycode(vk);
            let modifiers = decode_modifiers(event.modifierFlags());
            let is_repeat = event.isARepeat();
            let chars = event
                .characters()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let key = decode_key(code, &chars);
            let _ = is_visible_text; // retained for keyUp; suppress unused-fn warning here.
            // IME-first routing while composing. If a composition is
            // already live (`was_composing == true`), do NOT pre-dispatch
            // `Event::KeyDown` — the IME may consume the key for
            // composition purposes (e.g. Pinyin Tab cycles tone marks),
            // and the framework's focus/shortcut logic must not race the
            // IME. Stash the decoded key and let `interpretKeyEvents:`
            // run first; `doCommandBySelector:` re-emits the `KeyDown`
            // only if the IME refused the key.
            //
            // Non-composing keystrokes keep today's order (`KeyDown` →
            // `interpretKeyEvents:`) so plain typing + Cmd-shortcuts are
            // unaffected (9a back-compat).
            let arr = NSArray::from_slice(&[event]);
            if self.ivars().was_composing.get() {
                *self.ivars().pending_keydown.borrow_mut() = Some(PendingKey {
                    code,
                    key,
                    modifiers,
                    is_repeat,
                });
                self.interpretKeyEvents(&arr);
                // Clear after the dispatch: either `doCommandBySelector:`
                // already consumed it (None now), or the IME consumed the
                // key via setMarkedText:/insertText: and we want to drop
                // the stashed event so no spurious KeyDown fires later.
                self.ivars().pending_keydown.borrow_mut().take();
            } else {
                dispatch_event(Event::KeyDown {
                    window: id,
                    code,
                    key,
                    modifiers,
                    is_repeat,
                });
                self.interpretKeyEvents(&arr);
            }
        });
    }

    pub(super) fn responder_key_up(&self, event: &NSEvent) {
        let id = self.ivars().window_id.get();
        ffi_boundary(|| {
            let vk = event.keyCode();
            let code = keymap::decode_keycode(vk);
            let modifiers = decode_modifiers(event.modifierFlags());
            let chars = event
                .characters()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let key = decode_key(code, &chars);
            dispatch_event(Event::KeyUp {
                window: id,
                code,
                key,
                modifiers,
            });
        });
    }

    pub(super) fn responder_flags_changed(&self, event: &NSEvent) {
        let id = self.ivars().window_id.get();
        ffi_boundary(|| {
            let new_flags = event.modifierFlags();
            let prev_flags = self.ivars().prev_modifier_flags.replace(new_flags);
            emit_modifier_changes(id, prev_flags, new_flags);
        });
    }

    /// Reinstall the tracking area covering current view bounds. Called by
    /// AppKit when bounds change.
    pub(super) fn responder_update_tracking_areas(&self) {
        // Call super first
        // SAFETY: super signature is `- (void)updateTrackingAreas`.
        let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };

        // Remove old tracking area if present
        {
            let mut ta = self.ivars().tracking_area.borrow_mut();
            if let Some(old) = ta.take() {
                self.removeTrackingArea(&old);
            }
        }

        // Install new tracking area covering current bounds
        let bounds = self.bounds();
        let options = NSTrackingAreaOptions::ActiveAlways
            | NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::InVisibleRect;

        let tracking_area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                bounds,
                options,
                Some(self),
                None,
            )
        };
        self.addTrackingArea(&tracking_area);
        *self.ivars().tracking_area.borrow_mut() = Some(tracking_area);
    }
}

impl MetalView {
    /// `doCommandBySelector:` body. AppKit calls this when the IME refused
    /// the keystroke.
    ///
    /// - Composing path (`keyDown:` deferred the `KeyDown` dispatch): emit
    ///   the stashed `Event::KeyDown` now so navigation / Escape /
    ///   IMEs-that-pass-Tab-through still reach the framework. This is also
    ///   what makes the "Tab commits then focuses" flow work for IMEs that
    ///   commit-on-Tab — by the time AppKit reaches
    ///   `doCommandBySelector:(insertTab:)`, the prior `insertText:` already
    ///   cleared `was_composing` and emitted `ImeCommit`, so the synthesized
    ///   `KeyDown{Tab}` moves focus *after* the commit on a single press.
    /// - Non-composing path: `keyDown:` already dispatched `KeyDown`, so
    ///   `pending_keydown` is `None` and this is a no-op.
    pub(super) fn responder_do_command_by_selector(&self) {
        let id = self.ivars().window_id.get();
        if let Some(pk) = self.ivars().pending_keydown.borrow_mut().take() {
            dispatch_event(Event::KeyDown {
                window: id,
                code: pk.code,
                key: pk.key,
                modifiers: pk.modifiers,
                is_repeat: pk.is_repeat,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// WindowDelegate responder method bodies
// ---------------------------------------------------------------------------

impl WindowDelegate {
    /// Snapshot the current modifier flags onto the content view so
    /// `flagsChanged:` diffs are aligned with the actual hardware state —
    /// absorbs cross-app modifier drift (e.g. Shift pressed in another app
    /// during Alt-Tab).
    pub(super) fn responder_window_did_become_key(&self, notification: &NSNotification) {
        let window_id = self.ivars().window_id.get();
        ffi_boundary(|| {
            let Some(win) = notification
                .object()
                .and_then(|obj| obj.downcast::<NSWindow>().ok())
            else {
                return;
            };
            let Some(view) = win.contentView() else {
                return;
            };
            let Ok(metal_view) = view.downcast::<MetalView>() else {
                return;
            };
            let mtm = MainThreadMarker::new().expect("windowDidBecomeKey: invoked off main thread");
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            if let Some(event) = app.currentEvent() {
                metal_view
                    .ivars()
                    .prev_modifier_flags
                    .set(event.modifierFlags());
            }
            // Notify the framework so it can swap this window's native menu into
            // the single shared app menu bar (per-window menu ownership). macOS
            // has one `NSApp.mainMenu`, so the focused window's menu is installed
            // on focus rather than per window. Re-entrancy-safe: `dispatch_event`
            // drops the event if a handler is already on the stack.
            dispatch_event(Event::WindowFocused { window: window_id });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_position_does_not_divide_by_scale() {
        // bounds 400 high, click at logical (300, 100) from window-origin (Y-up)
        // Pre-fix: returned (300/2, (400-100)/2) = (150, 150) — bug
        // Post-fix: returns (300, 300) — Y-flip only, no scale division
        assert_eq!(
            decode_position_pure((300.0, 100.0), 400.0, 2.0),
            (300.0, 300.0)
        );
    }

    #[test]
    fn decode_position_flips_y_correctly() {
        // At scale=1.0 (no division anyway), verify Y flip.
        // Click at (50, 50) in Y-up coords with bounds_height=100
        // Expected: (50, 100-50) = (50, 50)
        assert_eq!(decode_position_pure((50.0, 50.0), 100.0, 1.0), (50.0, 50.0));
        // Click at (50, 0) in Y-up coords (bottom of view)
        // Expected: (50, 100-0) = (50, 100)
        assert_eq!(decode_position_pure((50.0, 0.0), 100.0, 1.0), (50.0, 100.0));
    }
}
