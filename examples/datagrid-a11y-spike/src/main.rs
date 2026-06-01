//! datagrid-a11y-spike — S0 DataGrid accessibility spike (macOS VoiceOver).
//!
//! Throwaway harness to validate the hardest screen-reader surface *before*
//! designing the real `DataGrid`: a 2D grid with `grid` / `row` / `gridcell` /
//! `columnheader` roles, zero-based cell indices, and 2D arrow-key navigation,
//! driven through Slate's real macOS AccessKit adapter.
//!
//! ## How to run the spike (human-in-the-loop)
//!
//! 1. `cargo run -p datagrid-a11y-spike`
//! 2. Turn VoiceOver on (Cmd-F5).
//! 3. Press Tab to focus the grid, then Arrow keys / Home / End / PageUp-Down
//!    to move the active cell.
//! 4. Listen: VoiceOver should announce the cell label, its role (column
//!    header vs cell), and "row R of N, column C of M".
//!
//! Report what VoiceOver actually says — that feeds the locked role/nav pattern
//! and the realistic P7 effort estimate.

use slate_framework::elements::data_grid_a11y_spike::data_grid_a11y_spike;
use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, App, AppContext, Color, Div, FlexDirection, IntoAny, Text, View,
    WindowOptions,
};

/// Data rows (a header row is added by the grid → 5 total grid rows).
const DATA_ROWS: usize = 4;
const COLS: usize = 3;

struct SpikeView {
    /// Active cell `(row, col)` in total-grid coords (row 0 = header).
    active: Signal<(usize, usize)>,
}

/// Human-readable label mirroring the grid's own `cell_label`, for the status
/// line (the grid owns the authoritative a11y labels).
fn active_label(row: usize, col: usize) -> String {
    if row == 0 {
        format!("Column {} (header)", col + 1)
    } else {
        format!("R{row}C{}", col + 1)
    }
}

impl View for SpikeView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let active = self.active.clone();
        let status = self.active.clone();

        Div::new()
            .background(Color::from_hex("#0d0d12").unwrap_or(Color::BLACK))
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .align_items(AlignItems::Center)
                    .gap(16.0)
                    .padding_all(24.0)
                    .flex_grow(1.0)
            })
            .child(
                Text::new("DataGrid a11y spike · Tab to focus, Arrows to navigate")
                    .font_size(16.0)
                    .color(Color::WHITE.into()),
            )
            .child(data_grid_a11y_spike(DATA_ROWS, COLS, active))
            .child(
                Text::new_reactive(move || {
                    let (r, c) = status.get();
                    format!("active: {}", active_label(r, c))
                })
                .font_size(13.0)
                .color(Color::from_hex("#94a3b8").unwrap_or(Color::WHITE).into()),
            )
            .into_any()
    }
}

fn main() {
    env_logger::init();

    App::new(WindowOptions {
        title: "Slate · datagrid-a11y-spike".into(),
        size: (520, 360),
        min_size: Some((420, 300)),
        resizable: true,
        ..Default::default()
    })
    .run(|cx: &AppContext| SpikeView {
        active: Signal::new(cx.runtime(), (0usize, 0usize)),
    });
}
