# DataGrid

**`DataGrid`** is a 2-D, virtualized table with a sticky header, uniform row
height, and configurable column widths. It registers an accessibility node for
every logical cell (cheap) while shaping glyphs only for the visible window.

## Columns

```rust
use slate_framework::Column;

fn grid_columns() -> Vec<Column> {
    vec![
        Column::new("PID", 70.0),
        Column::new("Process", 220.0),
        Column::new("CPU %", 90.0),
        Column::new("Mem MB", 100.0),
    ]
}
```

> Source: [`examples/dashboard/src/data.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/dashboard/src/data.rs).

## Usage

```rust
use slate_framework::DataGrid;

DataGrid::new(
    data::grid_columns(),         // Vec<Column>
    data::grid_rows(),            // Vec<Vec<String>>
    self.grid_active.clone(),     // Signal<(usize, usize)> active cell
    self.grid_offset.clone(),     // Signal<f32> scroll top
)
.height(320.0)
.label("Processes")
.selected(self.grid_selected.clone())  // Signal<Option<usize>> selected row
```

> Source: [`examples/dashboard/src/panels.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/dashboard/src/panels.rs).

## Key props

| Builder | Effect |
|---|---|
| `DataGrid::new(columns, rows, active, offset)` | Columns, row data, `Signal<(usize,usize)>` active cell, `Signal<f32>` scroll. |
| `Column::new(label, width)` | A column header + width in points. |
| `.height(f32)` | Fixed viewport height (enables virtualization). |
| `.label(name)` | Accessible name for the grid. |
| `.selected(Signal<Option<usize>>)` | Optional selected-row signal. |

## Accessibility

- **Role:** `Grid` container → `Row` rows → `ColumnHeader` / `Cell` cells. (The
  data cells use the interactive grid-cell role.)
- **Cell semantics:** each cell carries zero-based `row_index` / `column_index`;
  the grid reports the **full logical** `row_count` / `column_count` (including
  the header row) — not the windowed count — so off-screen navigation works.
- **Keyboard:** the grid is the tab stop; **Arrow / Home / End / PageUp /
  PageDown** move the active cell; the first arrow pulls focus onto a cell;
  **Enter** selects the row. Because every logical cell is registered, Page/Home/
  End can jump focus onto off-window cells.
