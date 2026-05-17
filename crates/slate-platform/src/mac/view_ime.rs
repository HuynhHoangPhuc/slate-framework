//! IME (NSTextInputClient) support for `MetalView`.
//!
//! Phase 9c: AppKit routes Input Method composition (Pinyin, Hiragana, dead
//! keys, emoji picker) through the `NSTextInputClient` protocol. This file
//! holds the helpers and the per-method body delegates; the protocol method
//! declarations themselves live in [`view::MetalView`]'s `define_class!`
//! block (objc2 requires protocol methods to be declared inline).
//!
//! All bodies are invoked on the main thread inside [`super::ffi_boundary`].

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, ClassType, DefinedClass};
use objc2_foundation::{
    NSArray, NSAttributedString, NSAttributedStringKey, NSNotFound, NSPoint, NSRange, NSRect,
    NSString, NSUInteger,
};

use super::view::MetalView;
use super::{dispatch_event, with_window_ime_delegate};
use crate::{Event, PhysicalRect};

// ---------------------------------------------------------------------------
// String bridge
// ---------------------------------------------------------------------------

/// Convert an `NSString` or `NSAttributedString` parameter from a
/// `NSTextInputClient` callback into a Rust `String`.
///
/// AppKit passes either type for `insertText:` and `setMarkedText:`; the
/// protocol declares the parameter as `id`. We probe `isKindOfClass:` to
/// pick the right accessor.
///
/// # Safety
///
/// `obj` must be a live `NSString` or `NSAttributedString` instance.
pub(super) unsafe fn ns_input_to_string(obj: &AnyObject) -> String {
    // SAFETY: `isKindOfClass:` is defined on NSObject and is safe for any
    // valid Obj-C instance. `NSAttributedString::class()` returns a stable
    // Class pointer.
    let attr_cls = NSAttributedString::class();
    let is_attr: bool = unsafe { objc2::msg_send![obj, isKindOfClass: attr_cls] };
    if is_attr {
        // SAFETY: confirmed NSAttributedString; `-string` returns an autoreleased NSString.
        let ns: Retained<NSString> = unsafe { objc2::msg_send![obj, string] };
        ns.to_string()
    } else {
        // SAFETY: caller contract: not an NSAttributedString → it's an NSString.
        let ns: &NSString = unsafe { &*(obj as *const AnyObject as *const NSString) };
        ns.to_string()
    }
}

/// Convert a Rust byte range to an `NSRange` (UTF-16 indices passed
/// transparently — the framework cache stores UTF-16-compatible offsets
/// for IME-facing queries in Phase 4).
///
/// Returns the canonical "not found" sentinel for `None`.
pub(super) fn range_to_nsrange(range: Option<core::ops::Range<usize>>) -> NSRange {
    match range {
        Some(r) => NSRange::new(r.start, r.end.saturating_sub(r.start)),
        None => NSRange::new(NSNotFound as usize, 0),
    }
}

// ---------------------------------------------------------------------------
// Coordinate conversion (HiDPI Y-flip)
// ---------------------------------------------------------------------------

/// Convert a screen-coord physical-pixel rect (framework convention) to a
/// logical-point screen-coord `NSRect` (AppKit convention).
///
/// Steps: divide by `scale` to drop the HiDPI factor, then Y-flip relative
/// to `screen_height_pt` because `NSScreen` has bottom-left origin while
/// the framework cache uses top-left.
///
/// The result is returned directly to AppKit — no further
/// `convertRect:toView:nil` / `window.convertRectToScreen:` call is needed
/// (the cache value is already in screen coords).
pub(super) fn physical_screen_rect_to_logical_nsrect(
    rect: PhysicalRect,
    screen_height_pt: f64,
    scale: f64,
) -> NSRect {
    if scale <= 0.0 {
        return NSRect::ZERO;
    }
    let x_pt = rect.x as f64 / scale;
    let y_pt = rect.y as f64 / scale;
    let w_pt = rect.width as f64 / scale;
    let h_pt = rect.height as f64 / scale;
    // Flip: AppKit Y starts at the bottom of the primary screen. Subtract
    // the caret's top edge from the screen height, then subtract the
    // caret's own height so the resulting NSRect.origin.y points to the
    // caret's bottom-left corner (AppKit convention).
    let y_appkit = screen_height_pt - y_pt - h_pt;
    NSRect::new(
        objc2_foundation::NSPoint::new(x_pt, y_appkit),
        objc2_foundation::NSSize::new(w_pt, h_pt),
    )
}

// ---------------------------------------------------------------------------
// Per-method delegates — invoked from the NSTextInputClient protocol body
// in `view.rs`. Kept in this file so `view.rs` stays focused on Obj-C glue.
// ---------------------------------------------------------------------------

impl MetalView {
    /// `insertText:replacementRange:` body.
    ///
    /// Distinguishes IME commit from non-IME typing via the
    /// `was_composing` flag (set by prior `setMarkedText:`). Atomically
    /// reads-and-clears the flag so the next non-IME keystroke goes back
    /// to the `Event::TextInput` path.
    pub(super) fn ime_handle_insert_text(&self, string: &AnyObject, _replacement: NSRange) {
        // SAFETY: NSTextInputClient guarantees string is NSString | NSAttributedString.
        let text = unsafe { ns_input_to_string(string) };
        let window = self.ivars().window_id.get();
        let was = self.ivars().was_composing.replace(false);
        if was {
            dispatch_event(Event::ImeCommit {
                window,
                text: text.clone(),
            });
            dispatch_event(Event::ImeDisabled { window });
        } else if !text.is_empty() {
            // Non-IME path: emoji-picker insertion and plain ASCII typing.
            dispatch_event(Event::TextInput { window, text });
        }
    }

    /// `setMarkedText:selectedRange:replacementRange:` body.
    ///
    /// Opens an IME session on the first call (`was_composing == false`
    /// when entered). Empty marked text combined with an active session
    /// is treated as "clear preedit" and emits the commit+disabled pair.
    pub(super) fn ime_handle_set_marked_text(
        &self,
        string: &AnyObject,
        selected: NSRange,
        _replacement: NSRange,
    ) {
        // SAFETY: NSTextInputClient guarantees string is NSString | NSAttributedString.
        let text = unsafe { ns_input_to_string(string) };
        let window = self.ivars().window_id.get();

        let opening = !self.ivars().was_composing.get();
        if opening && !text.is_empty() {
            self.ivars().was_composing.set(true);
            dispatch_event(Event::ImeEnabled { window });
        }

        if text.is_empty() {
            // Empty marked text mid-session → IME canceled the composition.
            if self.ivars().was_composing.replace(false) {
                dispatch_event(Event::ImeCommit {
                    window,
                    text: String::new(),
                });
                dispatch_event(Event::ImeDisabled { window });
            }
            return;
        }

        let selection = if selected.location == NSNotFound as usize {
            None
        } else {
            let start = selected.location;
            let end = start.saturating_add(selected.length);
            Some(start..end)
        };
        let cursor_byte_offset = selection
            .as_ref()
            .map(|r| r.start)
            .unwrap_or_else(|| text.len());

        dispatch_event(Event::ImePreedit {
            window,
            text,
            cursor_byte_offset,
            selection,
        });
    }

    /// `unmarkText` body — caller cancelled composition without committing.
    /// Emits the canonical empty-commit + disabled pair so framework
    /// listeners clear the preedit overlay without inserting any text.
    pub(super) fn ime_handle_unmark_text(&self) {
        if !self.ivars().was_composing.replace(false) {
            return;
        }
        let window = self.ivars().window_id.get();
        dispatch_event(Event::ImeCommit {
            window,
            text: String::new(),
        });
        dispatch_event(Event::ImeDisabled { window });
    }

    /// `selectedRange` body — sync query to framework cache.
    pub(super) fn ime_handle_selected_range(&self) -> NSRange {
        let id = self.ivars().window_id.get();
        with_window_ime_delegate(id, |d| d.ime_selected_range(id))
            .flatten()
            .map(|r| range_to_nsrange(Some(r)))
            .unwrap_or_else(|| range_to_nsrange(None))
    }

    /// `markedRange` body — sync query to framework cache.
    pub(super) fn ime_handle_marked_range(&self) -> NSRange {
        let id = self.ivars().window_id.get();
        with_window_ime_delegate(id, |d| d.ime_marked_range(id))
            .flatten()
            .map(|r| range_to_nsrange(Some(r)))
            .unwrap_or_else(|| range_to_nsrange(None))
    }

    /// `hasMarkedText` body — uses the local `was_composing` flag so the
    /// answer agrees with the events we have actually emitted, not with a
    /// possibly-not-yet-published cache value.
    pub(super) fn ime_handle_has_marked_text(&self) -> bool {
        self.ivars().was_composing.get()
    }

    /// `attributedSubstringForProposedRange:actualRange:` body.
    ///
    /// Returns the framework-cached substring wrapped in a plain
    /// `NSAttributedString` (no styling). The IME uses this for context-
    /// aware reconversion.
    pub(super) fn ime_handle_attributed_substring(
        &self,
        range: NSRange,
    ) -> Option<Retained<NSAttributedString>> {
        let id = self.ivars().window_id.get();
        let start = range.location;
        let end = start.saturating_add(range.length);
        let text = with_window_ime_delegate(id, |d| d.ime_text(id, start..end)).flatten()?;
        let ns_str = NSString::from_str(&text);
        Some(NSAttributedString::initWithString(
            NSAttributedString::alloc(),
            &ns_str,
        ))
    }

    /// `validAttributesForMarkedText` — empty array (no custom markup).
    pub(super) fn ime_handle_valid_attributes(&self) -> Retained<NSArray<NSAttributedStringKey>> {
        NSArray::new()
    }

    /// `firstRectForCharacterRange:actualRange:` body.
    ///
    /// Performs the HiDPI + Y-flip conversion documented on
    /// [`physical_screen_rect_to_logical_nsrect`]. AppKit uses the result
    /// to position the candidate / suggestion window. Returns the
    /// zero-rect if no caret is currently tracked (the OS will fall back
    /// to a screen-default placement, which is acceptable for v1).
    pub(super) fn ime_handle_first_rect(&self, _range: NSRange) -> NSRect {
        let id = self.ivars().window_id.get();
        let Some(rect_phys) = with_window_ime_delegate(id, |d| d.ime_caret_rect(id)).flatten()
        else {
            return NSRect::ZERO;
        };

        // Resolve screen geometry: use the view's window's screen if
        // available, falling back to the main screen.
        let mtm = match objc2_foundation::MainThreadMarker::new() {
            Some(m) => m,
            None => return NSRect::ZERO,
        };
        let screen = self
            .window()
            .and_then(|w| w.screen())
            .or_else(|| objc2_app_kit::NSScreen::mainScreen(mtm));
        let Some(screen) = screen else {
            return NSRect::ZERO;
        };
        let screen_frame = screen.frame();
        let scale = self.window().map(|w| w.backingScaleFactor()).unwrap_or(1.0);

        physical_screen_rect_to_logical_nsrect(
            rect_phys,
            screen_frame.origin.y + screen_frame.size.height,
            scale,
        )
    }

    /// `characterIndexForPoint:` — IME-driven hit-test not supported in v1.
    /// Returning `NSNotFound` is the documented "I don't know" reply.
    pub(super) fn ime_handle_character_index(&self, _point: NSPoint) -> NSUInteger {
        NSNotFound as NSUInteger
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_rect_flips_y_at_1x() {
        // Screen is 1000 pt tall; caret is at (100, 200) sized 2×16 in
        // physical pixels @ 1× scale. AppKit Y origin = bottom, so
        // result.y = 1000 - 200 - 16 = 784.
        let nsrect =
            physical_screen_rect_to_logical_nsrect(PhysicalRect::new(100, 200, 2, 16), 1000.0, 1.0);
        assert_eq!(nsrect.origin.x, 100.0);
        assert_eq!(nsrect.origin.y, 784.0);
        assert_eq!(nsrect.size.width, 2.0);
        assert_eq!(nsrect.size.height, 16.0);
    }

    #[test]
    fn screen_rect_divides_by_scale_at_2x() {
        // 2× retina: physical (200, 400) sized 4×32 → logical (100, 200) sized 2×16.
        // Screen height 1000 pt; flipped y = 1000 - 200 - 16 = 784.
        let nsrect =
            physical_screen_rect_to_logical_nsrect(PhysicalRect::new(200, 400, 4, 32), 1000.0, 2.0);
        assert_eq!(nsrect.origin.x, 100.0);
        assert_eq!(nsrect.origin.y, 784.0);
        assert_eq!(nsrect.size.width, 2.0);
        assert_eq!(nsrect.size.height, 16.0);
    }

    #[test]
    fn screen_rect_zero_input_yields_zero_at_top_left() {
        // Zero-rect at origin: y_pt = 0, h_pt = 0 → y_appkit = 1000.
        let nsrect =
            physical_screen_rect_to_logical_nsrect(PhysicalRect::new(0, 0, 0, 0), 1000.0, 1.0);
        assert_eq!(nsrect.origin.x, 0.0);
        assert_eq!(nsrect.origin.y, 1000.0);
        assert_eq!(nsrect.size.width, 0.0);
        assert_eq!(nsrect.size.height, 0.0);
    }

    #[test]
    fn screen_rect_handles_zero_scale_defensively() {
        // Bad input: scale 0.0 would divide-by-zero; helper returns ZERO.
        let nsrect =
            physical_screen_rect_to_logical_nsrect(PhysicalRect::new(100, 200, 2, 16), 1000.0, 0.0);
        assert_eq!(nsrect.origin.x, 0.0);
        assert_eq!(nsrect.origin.y, 0.0);
    }

    #[test]
    fn range_to_nsrange_round_trip() {
        let r = range_to_nsrange(Some(5..12));
        assert_eq!(r.location, 5);
        assert_eq!(r.length, 7);
    }

    #[test]
    fn range_to_nsrange_none_yields_notfound_zero() {
        let r = range_to_nsrange(None);
        assert_eq!(r.location, NSNotFound as usize);
        assert_eq!(r.length, 0);
    }
}
