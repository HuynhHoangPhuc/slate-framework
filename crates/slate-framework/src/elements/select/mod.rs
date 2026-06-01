//! Select — a dropdown bound to a caller-owned selected-index `Signal`.
//!
//! A `Select` is a thin composition: an input-styled **trigger** (a themed
//! [`Div`] that reports its live rect via [`track_bounds`](Div::track_bounds))
//! plus, while open, an [`Overlay`] anchored beneath it containing a
//! [`MenuList`](crate::elements::menu::MenuList) of options. The overlay is
//! `modal(true).scrim(false)`: it keeps modal focus management (focus moves into
//! the list on open, is trapped for arrow-key roving, and is restored to the
//! trigger on close) and Esc / click-away dismissal, **without** dimming the app.
//!
//! State is caller-owned (Strategy A): `selected` (the chosen index — the
//! meaningful value), `open` (UI open/closed), and `anchor` (the trigger's
//! tracked rect). Create all three outside `render` so they survive the
//! whole-view rebuild. Choosing an option writes `selected` and clears `open`.

use crate::element::{AnyElement, IntoElement, Sealed};
use crate::elements::menu::{MenuEntry, MenuList};
use crate::elements::{Div, Overlay, Placement, ScrollView, Text};
use crate::reactive::Signal;
use crate::theme::theme;
use crate::types::Bounds;

use slate_platform::{Key, NamedKey};

/// A dropdown selecting one option, bound to a caller-owned `Signal<usize>`.
///
/// # Example
/// ```ignore
/// Select::new(selected.clone(), open.clone(), anchor.clone())
///     .options(["Low", "Medium", "High"])
///     .scroll(scroll.clone()) // optional: cap height + scroll long lists
/// ```
pub struct Select {
    selected: Signal<usize>,
    open: Signal<bool>,
    anchor: Signal<Bounds>,
    options: Vec<MenuEntry>,
    /// Optional scroll offset: when set, the option list is capped in height and
    /// scrolls; without it the list grows to fit all options.
    scroll: Option<Signal<f32>>,
    placement: Placement,
}

impl Select {
    /// Create a select bound to the caller-owned `selected`, `open`, and
    /// `anchor` signals. Add options with [`options`](Self::options).
    pub fn new(selected: Signal<usize>, open: Signal<bool>, anchor: Signal<Bounds>) -> Self {
        Self {
            selected,
            open,
            anchor,
            options: Vec::new(),
            scroll: None,
            placement: Placement::bottom(),
        }
    }

    /// Set the selectable options (any `&str` / `String` / [`MenuEntry`]).
    pub fn options<I, E>(mut self, options: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<MenuEntry>,
    {
        self.options = options.into_iter().map(Into::into).collect();
        self
    }

    /// Cap the dropdown height and make it scroll, bound to a caller-owned
    /// offset signal (logical px, `0` = top).
    pub fn scroll(mut self, offset: Signal<f32>) -> Self {
        self.scroll = Some(offset);
        self
    }

    /// Override the dropdown placement (default: below the trigger). The solver
    /// still flips/shifts at viewport edges.
    pub fn placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }

    /// Label of the current selection (clamped), or empty if there are none.
    fn current_label(&self, idx: usize) -> String {
        self.options.get(idx).map(|e| e.label.clone()).unwrap_or_default()
    }

    /// The input-styled trigger: surface fill, hairline border, label + chevron;
    /// reports its rect into `anchor`, toggles `open` on click / Enter / Down.
    fn trigger(&self, idx: usize) -> Div {
        let t = theme();
        let open_click = self.open.clone();
        let open_key = self.open.clone();
        Div::new()
            .background(t.surface)
            .corner_radius(t.radius.sm)
            .border(t.border, 1.0)
            .focusable(true)
            .a11y_label(self.current_label(idx))
            .track_bounds(self.anchor.clone())
            .on_click(move |_, _| open_click.set(true))
            .on_key_down(move |ev, _| {
                if matches!(
                    ev.key,
                    Key::Named(NamedKey::Enter | NamedKey::Space | NamedKey::ArrowDown)
                ) && !ev.is_repeat
                {
                    open_key.set(true);
                }
            })
            .style(|s| s.row().gap(t.spacing.md).padding(crate::types::Edges::new(
                crate::style::Length::Px(t.spacing.sm),
                crate::style::Length::Px(t.spacing.md),
                crate::style::Length::Px(t.spacing.sm),
                crate::style::Length::Px(t.spacing.md),
            )))
            .child(
                Text::new(self.current_label(idx))
                    .font_size(t.typography.body.size)
                    .color(t.fg.into())
                    .presentational(),
            )
            .child(
                Text::new("▾")
                    .font_size(t.typography.body.size)
                    .color(t.muted.into())
                    .presentational(),
            )
    }

    /// The open dropdown overlay: a themed surface wrapping the option MenuList,
    /// optionally inside a height-capped ScrollView.
    fn dropdown(&self, idx: usize) -> Overlay {
        let t = theme();
        let close = self.open.clone();
        let selected = self.selected.clone();
        let open = self.open.clone();
        let menu = MenuList::new(self.options.clone())
            .initial_active(idx)
            .selected(Some(idx))
            .on_activate(move |i, _cx| {
                selected.set(i);
                open.set(false);
            });

        let body: AnyElement = match &self.scroll {
            Some(offset) => AnyElement::new(
                ScrollView::new()
                    .offset(offset.clone())
                    .style(|s| {
                        s.max_size(crate::style::SizeConstraint::new(
                            crate::style::Length::Auto,
                            crate::style::Length::Px(240.0),
                        ))
                    })
                    .child(menu),
            ),
            None => AnyElement::new(menu),
        };

        Overlay::new()
            .anchor(self.anchor.clone())
            .placement(self.placement)
            .modal(true)
            .scrim(false)
            .on_dismiss(move || close.set(false))
            .child(
                Div::new()
                    .background(t.surface)
                    .corner_radius(t.radius.md)
                    .border(t.border, 1.0)
                    .style(|s| {
                        s.column().padding_all(t.spacing.xs).min_size(
                            crate::style::SizeConstraint::new(
                                crate::style::Length::Px(160.0),
                                crate::style::Length::Auto,
                            ),
                        )
                    })
                    .child_any(body),
            )
    }
}

impl Sealed for Select {}

impl IntoElement for Select {
    type Element = Div;

    fn into_element(self) -> Div {
        let idx = self.selected.get(); // subscribe the render observer
        let open = self.open.get(); // subscribe so toggling rebuilds
        let idx = idx.min(self.options.len().saturating_sub(1));

        // Single wrapper: the trigger sizes the flow; the overlay (absolute) is a
        // sibling child that never disturbs it.
        let mut root = Div::new().style(|s| s.column()).child(self.trigger(idx));
        if open {
            root = root.child(self.dropdown(idx));
        }
        root
    }
}
