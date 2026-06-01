//! Button / IconButton constructors and builders.

use std::sync::Arc;

use slate_platform::{Key, NamedKey};

use crate::element::{AnyElement, IntoElement};
use crate::elements::Text;
use crate::event::{ElementKeyHandler, EventCtx, KeyEvent};
use crate::theme::theme;

use super::{Activate, Button};

impl Button {
    /// Create a labelled text button. The label is both the visible text and the
    /// accessible name. Colours come from the ambient [`theme`].
    pub fn new(label: impl Into<String>) -> Self {
        let label = label.into();
        let t = theme();
        // Presentational: the Button node already carries this string as its
        // accessible name, so the label must not emit its own `Label` node.
        let text = Text::new(label.clone())
            .font_size(t.typography.body.size)
            .color(t.bg.into())
            .presentational();
        Button {
            content: Some(AnyElement::new(text)),
            a11y_label: label,
            on_activate: None,
            disabled: false,
            fill: t.accent.into(),
            fill_disabled: t.muted.into(),
            corner_radius: t.radius.md,
            pad_x: t.spacing.md,
            pad_y: t.spacing.sm,
            last_id: None,
        }
    }

    /// Register the activation callback, fired on click and on Enter / Space.
    ///
    /// The callback receives only the [`EventCtx`]; a button press carries no
    /// meaningful per-event payload (unlike a raw mouse handler).
    pub fn on_click<F>(mut self, handler: F) -> Self
    where
        F: Fn(&mut EventCtx) + Send + Sync + 'static,
    {
        self.on_activate = Some(Arc::new(handler));
        self
    }

    /// Mark the button disabled: it registers no handlers/focus, paints the
    /// `muted` fill, and reports `is_disabled` to screen readers.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// Icon-only button. Construct via [`IconButton::new`], which returns a
/// configured [`Button`] — the icon variant differs only in content (an icon
/// element instead of text) and in requiring an explicit accessible name.
pub struct IconButton;

impl IconButton {
    /// Create an icon-only button. `aria_label` is **mandatory** — the icon has
    /// no readable text, so a missing label would leave the control nameless to
    /// screen readers. Returns a [`Button`]; chain `.on_click` / `.disabled`.
    // Intentionally returns `Button`, not `Self`: `IconButton` is a constructor
    // namespace, not a distinct element type.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(icon: impl IntoElement, aria_label: impl Into<String>) -> Button {
        let t = theme();
        Button {
            content: Some(AnyElement::new(icon)),
            a11y_label: aria_label.into(),
            on_activate: None,
            disabled: false,
            fill: t.accent.into(),
            fill_disabled: t.muted.into(),
            corner_radius: t.radius.md,
            pad_x: t.spacing.sm,
            pad_y: t.spacing.sm,
            last_id: None,
        }
    }
}

/// Build the Enter/Space activation key handler that wraps a button's
/// [`Activate`] callback. Ignores OS auto-repeat and stops propagation so the
/// key doesn't also drive Tab navigation.
pub(super) fn activation_key_handler(activate: Activate) -> ElementKeyHandler {
    Arc::new(move |ev: &KeyEvent, cx: &mut EventCtx| {
        let is_activate = matches!(
            ev.key,
            Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space)
        );
        if is_activate && !ev.is_repeat {
            activate(cx);
            cx.stop_propagation();
        }
    })
}
