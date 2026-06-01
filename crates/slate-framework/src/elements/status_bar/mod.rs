//! StatusBar — a bottom strip of status segments.
//!
//! A thin composition over [`Div`]: a `surface` row of text/indicator segments
//! with token padding + gap, carrying an (unlabelled) `AccessibilityRole::Group`
//! whose segments are read individually. Use [`text`](StatusBar::text) for a
//! plain muted-caption segment or [`child`](StatusBar::child) for an arbitrary
//! indicator element. It invents no new layout.

use crate::element::{AnyElement, IntoElement, Sealed};
use crate::elements::{Div, Text};
use crate::theme::theme;

/// A bottom status strip.
///
/// # Example
/// ```ignore
/// StatusBar::new()
///     .text("128 processes")
///     .separator()
///     .text("CPU 12%")
/// ```
#[derive(Default)]
pub struct StatusBar {
    items: Vec<AnyElement>,
}

impl StatusBar {
    /// Create an empty status bar.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a plain text segment (muted caption styling).
    pub fn text(mut self, text: impl Into<String>) -> Self {
        let t = theme();
        self.items.push(AnyElement::new(
            Text::new(text)
                .font_size(t.typography.caption.size)
                .color(t.muted.into()),
        ));
        self
    }

    /// Add an arbitrary indicator element as a segment.
    pub fn child<E: IntoElement>(mut self, child: E) -> Self {
        self.items.push(AnyElement::new(child));
        self
    }

    /// Add a type-erased segment.
    pub fn child_any(mut self, child: AnyElement) -> Self {
        self.items.push(child);
        self
    }

    /// Add a thin vertical separator between segments.
    pub fn separator(mut self) -> Self {
        let t = theme();
        self.items.push(AnyElement::new(
            Div::new()
                .background(t.border)
                .style(|s| s.width(1.0).height(14.0)),
        ));
        self
    }
}

impl Sealed for StatusBar {}

impl IntoElement for StatusBar {
    type Element = Div;

    fn into_element(self) -> Div {
        let t = theme();
        let mut row = Div::new()
            .background(t.surface)
            .border(t.border, 1.0)
            .style(|s| {
                s.row()
                    .align_items(taffy::style::AlignItems::Center)
                    .gap(t.spacing.md)
                    .padding(crate::types::Edges::symmetric(
                        crate::style::Length::Px(t.spacing.xs),
                        crate::style::Length::Px(t.spacing.md),
                    ))
            });
        for item in self.items {
            row = row.child_any(item);
        }
        row
    }
}
