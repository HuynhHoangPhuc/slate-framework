# Widget reference

One page per shipped widget. Each page gives the widget's purpose, a **real
snippet** copied from an `examples/` crate, its **accessibility notes** (role +
keyboard, from [`docs/a11y-contract.md`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/docs/a11y-contract.md)),
and its key signals/props.

Every interactive widget follows the same contract: **state is a caller-owned
`Signal`** you create once (outside `render`) and pass in. See
[Reactive signals](../concepts/reactive-signals.md).

## Form controls

- [Button & IconButton](button-and-icon-button.md)
- [Checkbox & Switch](checkbox-and-switch.md)
- [Slider](slider.md)
- [TextField & TextArea](text-field-and-text-area.md)

## Data display

- [List & VirtualList](list-and-virtual-list.md)
- [Tree](tree.md)
- [DataGrid](data-grid.md)
- [BarChart & Sparkline](charts.md)

## Containers & overlays

- [Layout containers](layout-containers.md) — Panel, Toolbar, StatusBar, Splitter
- [Overlay widgets](overlay-widgets.md) — Overlay, Select, Tooltip, ContextMenu, MenuList
- [Native menus](native-menus.md) — Menu, MenuItem, MenuAction, accelerators

## Where snippets come from

| Widget group | Example crate |
|---|---|
| Form controls | `examples/form-controls` |
| TextField / TextArea | `examples/ime-textfield`, `examples/textarea` |
| Overlay widgets | `examples/overlay-widgets`, `examples/overlay-popover` |
| Theme switching | `examples/theme-switch` |
| Native menus | `examples/native-menu` |
| Everything together | `examples/dashboard` |
