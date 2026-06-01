//! Toolbar — a horizontal strip hosting command controls.
//!
//! A thin composition over [`Div`]: a `surface` row that lays out
//! [`Button`](crate::Button)/[`IconButton`](crate::IconButton) controls (and
//! optional [`separator`](Toolbar::separator)s) with token padding + gap. It
//! carries an (unlabelled) `AccessibilityRole::Group` — the hosted controls
//! announce their own names — and invents no new layout (plain flexbox).

use crate::element::{AnyElement, IntoElement, Sealed};
use crate::elements::Div;
use crate::theme::theme;

/// A horizontal toolbar strip.
///
/// # Example
/// ```ignore
/// Toolbar::new()
///     .child(Button::new("Refresh").on_click(|_| refresh()))
///     .separator()
///     .child(Button::new("Settings"))
/// ```
#[derive(Default)]
pub struct Toolbar {
    items: Vec<AnyElement>,
}

impl Toolbar {
    /// Create an empty toolbar.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a control (typically a Button/IconButton).
    pub fn child<E: IntoElement>(mut self, child: E) -> Self {
        self.items.push(AnyElement::new(child));
        self
    }

    /// Add a type-erased control.
    pub fn child_any(mut self, child: AnyElement) -> Self {
        self.items.push(child);
        self
    }

    /// Add a thin vertical separator between groups of controls.
    pub fn separator(mut self) -> Self {
        let t = theme();
        self.items.push(AnyElement::new(
            Div::new()
                .background(t.border)
                .style(|s| s.width(1.0).height(20.0)),
        ));
        self
    }
}

impl Sealed for Toolbar {}

impl IntoElement for Toolbar {
    type Element = Div;

    fn into_element(self) -> Div {
        let t = theme();
        let mut row = Div::new()
            .background(t.surface)
            .border(t.border, 1.0)
            .style(|s| {
                s.row()
                    .align_items(taffy::style::AlignItems::Center)
                    .gap(t.spacing.sm)
                    .padding_all(t.spacing.sm)
            });
        for item in self.items {
            row = row.child_any(item);
        }
        row
    }
}
