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

use super::text_offset::{byte_to_utf16, utf16_to_byte};
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

/// The canonical `NSTextInputClient` "not found" range sentinel
/// (`{NSNotFound, 0}`), returned when no range is available.
pub(super) fn nsrange_not_found() -> NSRange {
    NSRange::new(NSNotFound as usize, 0)
}

/// Convert a **document byte** range to a UTF-16 `NSRange`, using `doc_prefix`
/// (the document text from byte 0 through `range.end`) to map both ends.
///
/// AppKit `NSRange` locations are UTF-16 code-unit offsets over the document;
/// the framework reports UTF-8 byte ranges. The prefix must reach `range.end`
/// so the mapping is exact; callers that cannot obtain it return
/// [`nsrange_not_found`] instead of guessing.
fn byte_range_to_nsrange_utf16(doc_prefix: &str, range: core::ops::Range<usize>) -> NSRange {
    let u_start = byte_to_utf16(doc_prefix, range.start);
    let u_end = byte_to_utf16(doc_prefix, range.end);
    NSRange::new(u_start, u_end.saturating_sub(u_start))
}

// ---------------------------------------------------------------------------
// Coordinate conversion (HiDPI + in-view Y-flip)
// ---------------------------------------------------------------------------

/// Convert a window-client-relative physical-pixel caret rect (framework
/// convention, top-left origin) into the `MetalView`'s own coordinate space
/// (AppKit-default bottom-left origin, logical points).
///
/// Steps: divide by `scale` to drop the HiDPI factor, then Y-flip relative
/// to `view_bounds_height_pt` because `MetalView` has no `isFlipped` override
/// (bottom-left origin) while the framework cache uses top-left. The MetalView
/// is the window's content view at origin (0,0), so the client-relative rect
/// maps directly into view space without a frame-origin offset.
///
/// This is only the in-view half of the conversion. The caller hands the
/// result to AppKit's `convertRect:toView:nil` + `window.convertRectToScreen:`
/// so AppKit adds the window origin / content inset and selects the correct
/// screen — no hand-rolled window-origin or multi-monitor arithmetic.
pub(super) fn physical_client_rect_to_view_nsrect(
    rect: PhysicalRect,
    view_bounds_height_pt: f64,
    scale: f64,
) -> NSRect {
    if scale <= 0.0 {
        return NSRect::ZERO;
    }
    let x_pt = rect.x as f64 / scale;
    let y_pt = rect.y as f64 / scale;
    let w_pt = rect.width as f64 / scale;
    let h_pt = rect.height as f64 / scale;
    // Flip: the view's Y origin is at its bottom. Subtract the caret's top
    // edge from the view height, then its own height so the resulting
    // NSRect.origin.y lands on the caret's bottom-left corner.
    let y_view = view_bounds_height_pt - y_pt - h_pt;
    NSRect::new(
        objc2_foundation::NSPoint::new(x_pt, y_view),
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

        // `selected` is a UTF-16 range **within the just-received marked
        // `text`**; the framework speaks UTF-8 bytes. Convert both ends against
        // `text` so the caret lands on the correct byte inside a CJK /
        // Vietnamese composition instead of a UTF-16-unit position.
        let selection = if selected.location == NSNotFound as usize {
            None
        } else {
            let start = utf16_to_byte(&text, selected.location);
            let end = utf16_to_byte(&text, selected.location.saturating_add(selected.length));
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

    /// `selectedRange` body — sync query to framework cache, byte→UTF-16.
    ///
    /// The cache returns a document **byte** range; AppKit wants UTF-16. We map
    /// it with the document prefix `[0..end]` obtained from `ime_text`. When
    /// the prefix is unavailable (caret far from the start of a long buffer
    /// that the windowed cache doesn't reach back to byte 0), we return the
    /// not-found sentinel rather than emit a wrong offset — the OS falls back
    /// to its own state.
    pub(super) fn ime_handle_selected_range(&self) -> NSRange {
        let id = self.ivars().window_id.get();
        with_window_ime_delegate(id, |d| match d.ime_selected_range(id) {
            Some(byte_range) => match d.ime_text(id, 0..byte_range.end) {
                Some(prefix) => byte_range_to_nsrange_utf16(&prefix, byte_range),
                None => nsrange_not_found(),
            },
            None => nsrange_not_found(),
        })
        .unwrap_or_else(nsrange_not_found)
    }

    /// `markedRange` body — sync query to framework cache, byte→UTF-16.
    ///
    /// Same byte→UTF-16 mapping as [`Self::ime_handle_selected_range`], but the
    /// not-found sentinel fires far more often here: the marked range ends at
    /// `caret + preedit_len`, while the cache's byte window is built only from
    /// the committed buffer (the preedit is overlaid, not spliced until
    /// commit). So during normal composition the prefix `[0..end]` can't reach
    /// `end` and we report not-found. Safe (no wrong offset, and AppKit set the
    /// marked text itself), but it means this query is effectively inert
    /// mid-composition until a preedit-inclusive / UTF-16-indexable accessor
    /// lands.
    pub(super) fn ime_handle_marked_range(&self) -> NSRange {
        let id = self.ivars().window_id.get();
        with_window_ime_delegate(id, |d| match d.ime_marked_range(id) {
            Some(byte_range) => match d.ime_text(id, 0..byte_range.end) {
                Some(prefix) => byte_range_to_nsrange_utf16(&prefix, byte_range),
                None => nsrange_not_found(),
            },
            None => nsrange_not_found(),
        })
        .unwrap_or_else(nsrange_not_found)
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
    ///
    /// LIMITATION: `range` is a UTF-16 range over the document; mapping it to
    /// the byte range `ime_text` expects requires reading the document text
    /// keyed by UTF-16 offset, which the windowed byte-keyed cache does not
    /// expose. We pass the offsets through as bytes — exact for ASCII context.
    /// For multibyte text it is usually safe (a non-`char`-boundary or
    /// out-of-window byte makes `ime_text` return `None`, so the OS falls back
    /// to its own state instead of a mis-sliced string), but NOT universally:
    /// a UTF-16 index that happens to land on a valid in-window byte boundary
    /// would yield a wrong-but-valid substring. Acceptable for the
    /// non-critical reconversion-context use; full UTF-16 mapping here is a
    /// follow-up gated on a UTF-16-indexable document accessor.
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
    /// The caret rect from the cache is window-client-relative physical px
    /// (top-left origin); AppKit expects screen-global points, bottom-left
    /// origin. We build the rect in the view's own space via
    /// [`physical_client_rect_to_view_nsrect`], then let AppKit add the window
    /// origin / content inset and pick the correct screen through
    /// `convertRect:toView:nil` + `window.convertRectToScreen:` — no manual
    /// window-origin or multi-monitor math. AppKit uses the result to position
    /// the candidate / suggestion window. Returns the zero-rect when no caret
    /// is tracked or no window is attached (the OS falls back to a
    /// screen-default placement, acceptable for v1).
    ///
    /// The `_range` arg (UTF-16 char range the IME asks about) is ignored: we
    /// always return the single tracked caret rect. Honoring sub-ranges would
    /// require mapping the range to per-glyph rects (multi-range support).
    ///
    /// **C11 (panel below the typed line):** the cached rect's `y_client`
    /// sits at the line top with `h_client = line_height`. After the in-view
    /// flip, the resulting NSRect's *top edge in screen coords* lands at the
    /// line top — and AppKit anchors the candidate panel against that top
    /// edge, which places the panel overlapping the typed text.
    ///
    /// We transform the rect into a 1-physical-pixel sliver positioned at
    /// the line *bottom* in client coords. The flip then yields an NSRect
    /// whose top edge in screen coords lands at the line bottom, so AppKit
    /// drops the panel below the line. Bypassing `_phys.height` for the
    /// returned size keeps the rect tiny — it can't extend off the view and
    /// be clamped to view top-left by AppKit's bounds-checking, which is
    /// what produced the earlier full-height variant's wrong placement.
    pub(super) fn ime_handle_first_rect(&self, _range: NSRange) -> NSRect {
        let id = self.ivars().window_id.get();
        let Some(rect_phys) = with_window_ime_delegate(id, |d| d.ime_caret_rect(id)).flatten()
        else {
            return NSRect::ZERO;
        };

        // Defense-in-depth: real producers always publish `Some(rect)` with
        // non-zero dims (TextField/TextArea paint), and the framework reports
        // `None` when nothing's been published (caught above). A true-zero
        // `Some` here would flip to the view's top-left after the in-view
        // flip — return ZERO so AppKit falls back to its screen-default
        // placement instead of anchoring the panel at the top-left corner.
        if rect_phys.width == 0 && rect_phys.height == 0 {
            return NSRect::ZERO;
        }

        // A live window is required for the view→window→screen conversion.
        let Some(window) = self.window() else {
            return NSRect::ZERO;
        };
        let scale = window.backingScaleFactor();
        if scale <= 0.0 {
            return NSRect::ZERO;
        }

        // Sliver at the line bottom: shift `y_client` down by the cached
        // height and shrink `h_client` to 1 physical pixel.
        let sliver = PhysicalRect::new(
            rect_phys.x,
            rect_phys.y.saturating_add(rect_phys.height as i32),
            rect_phys.width,
            1,
        );

        // Build the caret rect in the view's own coordinate space (points,
        // bottom-left origin), then convert view → window → screen.
        let view_bounds_height_pt = self.bounds().size.height;
        let rect_in_view =
            physical_client_rect_to_view_nsrect(sliver, view_bounds_height_pt, scale);
        let rect_in_window = self.convertRect_toView(rect_in_view, None);
        window.convertRectToScreen(rect_in_window)
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
    fn view_rect_flips_y_at_1x() {
        // View is 1000 pt tall; caret is at (100, 200) sized 2×16 in physical
        // pixels @ 1× scale. The view's Y origin = bottom, so
        // result.y = 1000 - 200 - 16 = 784. (Screen offset is added later by
        // AppKit's convertRectToScreen, not in this pure helper.)
        let nsrect =
            physical_client_rect_to_view_nsrect(PhysicalRect::new(100, 200, 2, 16), 1000.0, 1.0);
        assert_eq!(nsrect.origin.x, 100.0);
        assert_eq!(nsrect.origin.y, 784.0);
        assert_eq!(nsrect.size.width, 2.0);
        assert_eq!(nsrect.size.height, 16.0);
    }

    #[test]
    fn view_rect_divides_by_scale_at_2x() {
        // 2× retina: physical (200, 400) sized 4×32 → logical (100, 200) sized 2×16.
        // View height 1000 pt; flipped y = 1000 - 200 - 16 = 784.
        let nsrect =
            physical_client_rect_to_view_nsrect(PhysicalRect::new(200, 400, 4, 32), 1000.0, 2.0);
        assert_eq!(nsrect.origin.x, 100.0);
        assert_eq!(nsrect.origin.y, 784.0);
        assert_eq!(nsrect.size.width, 2.0);
        assert_eq!(nsrect.size.height, 16.0);
    }

    #[test]
    fn view_rect_zero_input_yields_zero_at_top_left() {
        // Zero-rect at origin: y_pt = 0, h_pt = 0 → y_view = 1000 (view top).
        let nsrect =
            physical_client_rect_to_view_nsrect(PhysicalRect::new(0, 0, 0, 0), 1000.0, 1.0);
        assert_eq!(nsrect.origin.x, 0.0);
        assert_eq!(nsrect.origin.y, 1000.0);
        assert_eq!(nsrect.size.width, 0.0);
        assert_eq!(nsrect.size.height, 0.0);
    }

    #[test]
    fn view_rect_handles_zero_scale_defensively() {
        // Bad input: scale 0.0 would divide-by-zero; helper returns ZERO.
        let nsrect =
            physical_client_rect_to_view_nsrect(PhysicalRect::new(100, 200, 2, 16), 1000.0, 0.0);
        assert_eq!(nsrect.origin.x, 0.0);
        assert_eq!(nsrect.origin.y, 0.0);
    }

    #[test]
    fn byte_range_to_nsrange_ascii_is_identity() {
        // ASCII prefix: byte offsets == UTF-16 offsets.
        let r = byte_range_to_nsrange_utf16("hello world", 5..11);
        assert_eq!(r.location, 5);
        assert_eq!(r.length, 6);
    }

    #[test]
    fn byte_range_to_nsrange_cjk_maps_to_utf16() {
        // "あい" = 2 UTF-16 units / 6 bytes. Byte range 3..6 (the 2nd char)
        // → UTF-16 range 1..2 → location 1, length 1.
        let r = byte_range_to_nsrange_utf16("あい", 3..6);
        assert_eq!(r.location, 1);
        assert_eq!(r.length, 1);
    }

    #[test]
    fn nsrange_not_found_is_notfound_zero() {
        let r = nsrange_not_found();
        assert_eq!(r.location, NSNotFound as usize);
        assert_eq!(r.length, 0);
    }
}
