//! Panel builders for the dashboard, split out of `main.rs` to keep each file
//! focused. Each method composes already-extracted P5 widgets — the dashboard is
//! assembled, not newly invented (the forcing-function payoff of Approach C).

use slate_framework::{
    AlignItems, BarChart, Button, Checkbox, ContextMenu, DataGrid, Div, IconButton, Panel, Select,
    Slider, Sparkline, StatusBar, Switch, Text, TextField, ThemeMode, Toolbar, Tooltip, Tree,
    VirtualList, theme,
};

use crate::data;
use crate::DashboardView;

/// A foreground-coloured body text line.
fn line(text: &str) -> Text {
    let t = theme();
    Text::new(text).font_size(t.typography.body.size).color(t.fg.into())
}

/// Flip the app light/dark.
fn toggle_mode(mode: &slate_framework::reactive::Signal<ThemeMode>) {
    mode.update(|m| {
        *m = match *m {
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::Light,
        }
    });
}

impl DashboardView {
    /// Top toolbar: actions, a tooltip-wrapped destructive button, light/dark.
    pub(crate) fn toolbar(&self) -> Toolbar {
        let mode = self.mode.clone();
        let theme_label = match self.mode.get() {
            ThemeMode::Light => "Dark mode",
            ThemeMode::Dark => "Light mode",
        };
        Toolbar::new()
            .child(Button::new("Refresh").on_click(|_| log::info!("Refresh clicked")))
            .child(IconButton::new("⟳", "Reload process list").on_click(|_| log::info!("Reload")))
            .separator()
            .child(
                Tooltip::new(self.tip_hover.clone(), "Force-quits the selected process")
                    .child(Button::new("End Process")),
            )
            .separator()
            .child(Button::new(theme_label).on_click(move |_| toggle_mode(&mode)))
    }

    /// Sidebar: process-hierarchy Tree above a flat process VirtualList.
    pub(crate) fn sidebar(&self) -> Panel {
        let t = theme();
        let tree = Tree::new(
            data::process_tree(),
            self.tree_expanded.clone(),
            self.tree_selected.clone(),
        )
        .label("Process hierarchy")
        .on_activate(|id, _| log::info!("tree node {id} activated"));

        let list = VirtualList::new(
            data::PROCESSES.iter().copied(),
            self.list_selected.clone(),
            self.list_offset.clone(),
        )
        .label("All processes")
        .height(180.0)
        .on_activate(|i, _| log::info!("process row {i} activated"));

        Panel::new("Processes").child(
            Div::new().style(|s| s.column().gap(t.spacing.md)).child(tree).child(list),
        )
    }

    /// Settings strip: the full form-control set + a Select, all caller-owned.
    pub(crate) fn settings(&self) -> Div {
        let t = theme();
        let sort = Select::new(
            self.sort_by.clone(),
            self.sort_open.clone(),
            self.sort_anchor.clone(),
        )
        .options(data::SORT_FIELDS.iter().copied())
        .scroll(self.sort_scroll.clone());

        Div::new()
            .style(|s| s.row().gap(t.spacing.lg).align_items(AlignItems::Center))
            .child(Div::new().style(|s| s.column().gap(t.spacing.xs)).child(line("Sort by")).child(sort))
            .child(Checkbox::new(self.show_system.clone(), "System processes"))
            .child(Switch::new(self.auto_refresh.clone(), "Auto-refresh"))
            .child(
                Slider::new(self.refresh_secs.clone(), 1.0, 10.0)
                    .step(1.0)
                    .label("Interval (s)")
                    .width(140.0),
            )
            .child(TextField::new(self.filter_text.clone()).themed())
    }

    /// Main area: the process DataGrid, primitive-drawn viz, and settings —
    /// right-click anywhere opens an in-canvas ContextMenu.
    pub(crate) fn main_pane(&self) -> Panel {
        let t = theme();
        let grid = DataGrid::new(
            data::grid_columns(),
            data::grid_rows(),
            self.grid_active.clone(),
            self.grid_offset.clone(),
        )
        .height(320.0)
        .label("Processes")
        .selected(self.grid_selected.clone());

        let viz = Div::new()
            .style(|s| s.row().gap(t.spacing.lg).align_items(AlignItems::FlexEnd))
            .child(BarChart::new(data::per_core_load()).size(180.0, 90.0).label("Per-core load"))
            .child(Sparkline::new(data::cpu_history()).size(220.0, 64.0).label("CPU history"));

        let last = self.last_action.clone();
        let menu_items = ["Inspect", "End Process", "Copy PID"];
        let body = Div::new()
            .style(|s| s.column().gap(t.spacing.sm))
            .child(line("Tab into the grid; arrows move the active cell, Enter selects a row."))
            .child(grid)
            .child(viz)
            .child(self.settings());
        let ctx = ContextMenu::new(self.ctx_open.clone(), self.ctx_anchor.clone())
            .items(menu_items)
            .on_select(move |i| last.set(format!("Action: {}", menu_items[i])))
            .child(body);

        Panel::new("Resource Detail").child(ctx)
    }

    /// Bottom status bar: synthetic totals + the last context-menu action.
    pub(crate) fn status(&self) -> StatusBar {
        StatusBar::new()
            .text(format!("{} processes", data::PROCESSES.len()))
            .separator()
            .text("CPU 12%")
            .separator()
            .text("Mem 4.2 GB")
            .separator()
            .text(self.last_action.get())
    }
}
