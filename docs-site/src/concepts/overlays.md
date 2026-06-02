# Overlays

An **overlay** is content that paints on top of the rest of the UI on a
high-depth scene layer: popovers, dropdowns, tooltips, and modal dialogs all
ride it. The `Overlay` element provides three things: an **anchor solver**, an
optional **modal scrim**, and **focus trapping**.

## Anchoring

An overlay is positioned relative to an **anchor rect** with a **placement**.
The anchor can be a static `Bounds` or a live `Signal<Bounds>` — pairing it with
`Div::track_bounds(signal)` makes the overlay follow an element as it moves
(anchor-to-element, no hardcoded coordinates):

```rust
fn popover(&self) -> Overlay {
    let open = self.open.clone();
    Overlay::new()
        .anchor(self.anchor.clone())      // a Signal<Bounds> written by track_bounds
        .placement(Placement::bottom())
        .on_dismiss(move || open.set(false))
        .child(/* popover body */)
}
```

```rust
// The trigger reports its live painted rect into the anchor signal:
Div::new()
    .track_bounds(self.anchor.clone())
    .on_click(move |_, _| open.update(|o| *o = !*o))
```

> Source: [`examples/overlay-popover/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/overlay-popover/src/main.rs).

The solver **flips** the placement automatically when there is no room (e.g. a
bottom-placed popover flips above its anchor near the window's bottom edge).

## Dismissal

`Overlay::on_dismiss(closure)` fires on **Esc** or a **click outside** the
overlay. For non-modal popovers, clicks on the anchor are excluded so the
trigger still toggles.

## Modal scrim + focus trap

`Overlay::modal(true)` turns the overlay into a dialog:

- dims the window behind it with a **scrim**,
- **traps Tab focus** to the overlay's focusable children (Tab/Shift+Tab cycle
  inside only),
- **auto-focuses** the first focusable child on open and **restores** the prior
  focus on close,
- **blocks** clicks from reaching the content behind it.

```rust
Overlay::new()
    .modal(true)
    .anchor(Bounds::from_origin_size(140.0, 90.0, 200.0, 0.0))
    .placement(Placement::bottom())
    .on_dismiss(move || dismiss.set(false))
    .child(/* dialog body with focusable buttons */)
```

> Source: [`examples/overlay-popover/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/overlay-popover/src/main.rs).

`Overlay::scrim(false)` keeps all modal focus/dismiss behavior but skips the
dimming paint — that is how the menu-style widgets (`Select`, `ContextMenu`)
trap focus without darkening the app.

## Showing an overlay conditionally

Overlays are laid out absolutely, so adding one as a child never disturbs the
surrounding layout. Gate it on a signal in `render`:

```rust
if self.open.get() {
    root = root.child(self.popover());
}
```

## Built on overlays

These widgets are overlay-backed — see their pages:
[`Overlay` / `Tooltip` / `ContextMenu` / `Select`](../widgets/overlay-widgets.md).
