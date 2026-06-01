//! [`VirtualList`] — a windowed, scrollable list with the same builder surface
//! as [`List`](super::List).
//!
//! It composes into a [`ScrollView::virtualized`](crate::ScrollView::virtualized)
//! whose row builder yields a self-describing [`VirtualRow`](super::virtual_row),
//! so per-frame cost is O(visible) regardless of length, while the container
//! reports the **true total** `row_count` to assistive tech via
//! [`ScrollView::a11y_list`](crate::ScrollView::a11y_list).
//!
//! ## Navigation model (and its documented limit)
//!
//! Because a windowed list never materializes off-screen rows, it cannot move a
//! single roving focus cursor onto a row that isn't built (focus only lands on
//! *registered* elements). So each **visible** row is an independent Tab stop and
//! click target, and the viewport scrolls via the wheel / PageUp-Down / Home-End
//! (inherited from [`ScrollView`](crate::ScrollView)). Single-cursor Up/Down
//! roving across the full virtual range — and its screen-reader "row X of N"
//! announcement under live VoiceOver — is the plan's validate-first item; if it
//! proves necessary the decided fallback is to emit every logical row as an a11y
//! node while keeping the visual window. Use [`List`](super::List) for full
//! roving over a bounded set.

use crate::element::{IntoElement, Sealed};
use crate::elements::ScrollView;
use crate::reactive::Signal;
use crate::theme::theme;

use super::core::ListEntry;
use super::virtual_row::{RowStyle, VirtualRow};
use super::ListActivate;
use std::sync::Arc;

/// A windowed, scrollable, single-selection list bound to caller-owned
/// `selected` and scroll-`offset` signals.
///
/// # Example
/// ```ignore
/// // `selected` and `offset` are created once (outside `render`).
/// VirtualList::new(rows, selected.clone(), offset.clone())
///     .height(400.0)        // required: a fixed viewport height windows the rows
///     .label("Processes")
/// ```
pub struct VirtualList {
    entries: Vec<ListEntry>,
    selected: Signal<Option<usize>>,
    offset: Signal<f32>,
    selected_value: Option<usize>,
    on_activate: ListActivate,
    label: Option<String>,
    /// Fixed viewport height in logical px (required to window the rows).
    height: f32,
    style: RowStyle,
}

impl VirtualList {
    /// Create a virtual list from any iterable of [`ListEntry`]-convertible
    /// items, bound to caller-owned `selected` and scroll `offset` signals.
    pub fn new<I, E>(items: I, selected: Signal<Option<usize>>, offset: Signal<f32>) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<ListEntry>,
    {
        let entries: Vec<ListEntry> = items.into_iter().map(Into::into).collect();
        let t = theme();
        let style = RowStyle {
            fg: t.fg.into(),
            muted: t.muted.into(),
            accent: t.accent.into(),
            font_size: t.typography.body.size,
            row_h: t.typography.body.size + t.spacing.md,
            pad_x: t.spacing.md,
        };
        let selected_value = selected.get(); // subscribe the render observer
        Self {
            entries,
            selected,
            offset,
            selected_value,
            on_activate: Arc::new(|_, _| {}),
            label: None,
            height: 320.0,
            style,
        }
    }

    /// Set the fixed viewport height (logical px). A virtualized list **needs**
    /// a pixel height to size its window; without one it renders every row.
    pub fn height(mut self, height: f32) -> Self {
        self.height = height.max(0.0);
        self
    }

    /// Set the accessible name announced for the list container.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the activation callback fired on row click after the selection
    /// signal is written.
    pub fn on_activate<F>(mut self, f: F) -> Self
    where
        F: Fn(usize, &mut crate::event::EventCtx) + Send + Sync + 'static,
    {
        self.on_activate = Arc::new(f);
        self
    }
}

impl Sealed for VirtualList {}

impl IntoElement for VirtualList {
    type Element = ScrollView;

    fn into_element(self) -> ScrollView {
        let VirtualList {
            entries,
            selected,
            offset,
            selected_value,
            on_activate,
            label,
            height,
            style,
        } = self;
        let count = entries.len();
        let row_h = style.row_h;
        let entries = Arc::new(entries);

        ScrollView::new()
            .offset(offset)
            .a11y_list(label, count)
            .style(move |s| s.height(height).column())
            .virtualized(count, row_h, move |i| {
                let entry = &entries[i];
                VirtualRow::new(
                    entry.label.clone(),
                    i,
                    entry.disabled,
                    selected_value == Some(i),
                    selected.clone(),
                    on_activate.clone(),
                    style,
                )
            })
    }
}
