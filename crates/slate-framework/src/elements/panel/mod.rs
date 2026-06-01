//! Panel — a titled surface container.
//!
//! A Panel is a thin composition over [`Div`]: a bordered `surface` card with a
//! presentational title header and a body slot. It encodes the standard theme
//! styling (surface fill, hairline border, `radius.md`) and the accessibility
//! identity (an `AccessibilityRole::Group` labelled by the title) so apps don't
//! re-derive a card every time. It invents no new layout — arrangement is
//! plain flexbox.
//!
//! The title `Text` is [presentational](crate::elements::Text::presentational)
//! so it emits no a11y node of its own; the group's label (the title) is the
//! single accessible name, avoiding a double announcement.
//!
//! Pass [`scroll`](Panel::scroll) a caller-owned offset signal to make the body
//! a [`ScrollView`](crate::ScrollView) (clipped + wheel/keyboard scrollable).

use crate::element::{AnyElement, IntoElement, Sealed};
use crate::elements::{Div, ScrollView, Text};
use crate::reactive::Signal;
use crate::theme::theme;

/// A titled surface container (header + body) styled from theme tokens.
///
/// # Example
/// ```ignore
/// Panel::new("Processes")
///     .child(process_table)         // body content
/// // scrollable variant:
/// Panel::new("Log").scroll(offset).child(long_list)
/// ```
pub struct Panel {
    title: String,
    body: Vec<AnyElement>,
    /// When set, the body is wrapped in a `ScrollView` bound to this offset.
    scroll: Option<Signal<f32>>,
}

impl Panel {
    /// Create a panel with the given title (also its accessible group name).
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: Vec::new(),
            scroll: None,
        }
    }

    /// Add a body child below the header.
    pub fn child<E: IntoElement>(mut self, child: E) -> Self {
        self.body.push(AnyElement::new(child));
        self
    }

    /// Add a type-erased body child.
    pub fn child_any(mut self, child: AnyElement) -> Self {
        self.body.push(child);
        self
    }

    /// Make the body scrollable, bound to the caller-owned vertical `offset`
    /// signal (logical px, `0` = top). Create the signal outside `render` so it
    /// survives the whole-view rebuild.
    pub fn scroll(mut self, offset: Signal<f32>) -> Self {
        self.scroll = Some(offset);
        self
    }
}

impl Sealed for Panel {}

impl IntoElement for Panel {
    type Element = Div;

    fn into_element(self) -> Div {
        let t = theme();

        // Presentational title: pixels only, no a11y node (the group label
        // carries the accessible name).
        let header = Text::new(&self.title)
            .font_size(t.typography.heading.size)
            .color(t.fg.into())
            .presentational();

        // Body fills the space below the header. Scrollable bodies wrap the
        // children in a clip-only ScrollView bound to the caller's offset.
        let body: AnyElement = match self.scroll {
            Some(offset) => {
                let mut sv = ScrollView::new().offset(offset).style(|s| s.flex_grow(1.0));
                for child in self.body {
                    sv = sv.child_any(child);
                }
                AnyElement::new(sv)
            }
            None => {
                let mut col =
                    Div::new().style(|s| s.column().gap(t.spacing.md).flex_grow(1.0));
                for child in self.body {
                    col = col.child_any(child);
                }
                AnyElement::new(col)
            }
        };

        Div::new()
            .background(t.surface)
            .corner_radius(t.radius.md)
            .border(t.border, 1.0)
            .a11y_label(self.title)
            .style(|s| {
                s.column()
                    .gap(t.spacing.md)
                    .padding_all(t.spacing.lg)
            })
            .child(header)
            .child_any(body)
    }
}
