//! dashboard — the P5b dev-tool/dashboard shell (process/resource monitor).
//!
//! Stands up the layout frame every later widget plugs into: a [`Toolbar`] on
//! top, a [`Splitter`] dividing a sidebar from the main area, and a
//! [`StatusBar`] on the bottom — all [`Panel`]s styled from ambient theme
//! tokens. The split is draggable (and Arrow-key resizable when the divider is
//! focused via Tab); the top-right button flips light/dark. Pane contents are
//! placeholders here — later phases replace them with the process Tree,
//! DataGrid, and viz widgets. Sample data is synthetic (no OS enumeration).
//!
//! Run: `cargo run -p dashboard`

use std::collections::HashSet;

use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, App, AppContext, Button, Div, FlexDirection, IntoAny, Panel, Splitter,
    StatusBar, Text, ThemeMode, Toolbar, Tree, TreeNode, View, VirtualList, WindowOptions, theme,
};

/// Synthetic process rows for the sidebar (no real OS enumeration — a fixed
/// fixture is enough to exercise the shell).
const PROCESSES: &[&str] = &[
    "kernel_task",
    "WindowServer",
    "launchd",
    "slate-framework",
    "Terminal",
    "Safari",
    "mds_stores",
    "cfprefsd",
];

/// Caller-owned shell state, created once in `App::run`.
struct DashboardView {
    mode: Signal<ThemeMode>,
    /// Sidebar/main split ratio (first-pane fraction), survives rebuilds.
    split: Signal<f32>,
    /// Expanded node ids of the process-hierarchy [`Tree`].
    tree_expanded: Signal<HashSet<u64>>,
    /// Selected node id of the process-hierarchy [`Tree`].
    tree_selected: Signal<Option<u64>>,
    /// Selected row of the flat process [`VirtualList`].
    list_selected: Signal<Option<usize>>,
    /// Scroll offset of the process [`VirtualList`].
    list_offset: Signal<f32>,
}

/// Build the synthetic process hierarchy shown in the sidebar Tree. Ids are
/// stable and unique — the Tree uses them as expansion/selection keys.
fn process_tree() -> Vec<TreeNode> {
    vec![
        TreeNode::new(0, "System").children([
            TreeNode::new(1, "kernel_task"),
            TreeNode::new(2, "WindowServer"),
            TreeNode::new(3, "launchd"),
            TreeNode::new(4, "mds_stores"),
        ]),
        TreeNode::new(10, "Applications").children([
            TreeNode::new(11, "slate-framework"),
            TreeNode::new(12, "Terminal"),
            TreeNode::new(13, "Safari"),
        ]),
    ]
}

fn toggle_mode(mode: &Signal<ThemeMode>) {
    mode.update(|m| {
        *m = match *m {
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::Light,
        }
    });
}

/// A muted-caption text line.
fn line(text: &str) -> Text {
    let t = theme();
    Text::new(text)
        .font_size(t.typography.body.size)
        .color(t.fg.into())
}

impl View for DashboardView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let t = theme();

        // --- Toolbar: actions + light/dark toggle ---
        let mode = self.mode.clone();
        let theme_label = match self.mode.get() {
            ThemeMode::Light => "Dark mode",
            ThemeMode::Dark => "Light mode",
        };
        let toolbar = Toolbar::new()
            .child(Button::new("Refresh").on_click(|_| log::info!("Refresh clicked")))
            .child(Button::new("Sort").on_click(|_| log::info!("Sort clicked")))
            .separator()
            .child(Button::new(theme_label).on_click(move |_| toggle_mode(&mode)));

        // --- Sidebar: process-hierarchy Tree above a flat process VirtualList ---
        let tree = Tree::new(
            process_tree(),
            self.tree_expanded.clone(),
            self.tree_selected.clone(),
        )
        .label("Process hierarchy")
        .on_activate(|id, _| log::info!("tree node {id} activated"));

        let list = VirtualList::new(
            PROCESSES.iter().copied(),
            self.list_selected.clone(),
            self.list_offset.clone(),
        )
        .label("All processes")
        .height(180.0)
        .on_activate(|i, _| log::info!("process row {i} activated"));

        let sidebar = Panel::new("Processes").child(
            Div::new()
                .style(|s| s.column().gap(t.spacing.md))
                .child(tree)
                .child(list),
        );

        // --- Main area: placeholder for the P5e DataGrid + P5f viz ---
        let main = Panel::new("Resource Detail").child(
            Div::new()
                .style(|s| s.column().gap(t.spacing.sm))
                .child(line("Select a process to inspect its resource usage."))
                .child(line("CPU, memory, and I/O panels land in a later phase.")),
        );

        let splitter = Splitter::new(self.split.clone())
            .first(sidebar)
            .second(main)
            .min_sizes(160.0, 320.0)
            .label("Sidebar / main divider");

        // --- StatusBar: totals (synthetic) ---
        let status = StatusBar::new()
            .text(format!("{} processes", PROCESSES.len()))
            .separator()
            .text("CPU 12%")
            .separator()
            .text("Mem 4.2 GB");

        Div::new()
            .background(t.bg)
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .align_items(AlignItems::Stretch)
                    .flex_grow(1.0)
            })
            .child(toolbar)
            .child(splitter)
            .child(status)
            .into_any()
    }
}

fn main() {
    env_logger::init();

    App::new(WindowOptions {
        title: "Slate · dashboard".into(),
        size: (920, 640),
        min_size: Some((640, 480)),
        resizable: true,
        ..Default::default()
    })
    .run(|cx: &AppContext| {
        let rt = cx.runtime();
        let mode = cx
            .theme_mode()
            .expect("theme_mode available inside App::run");
        DashboardView {
            mode,
            split: Signal::new(rt.clone(), 0.28),
            // Both hierarchy groups expanded so the demo opens populated.
            tree_expanded: Signal::new(rt.clone(), HashSet::from([0, 10])),
            tree_selected: Signal::new(rt.clone(), None),
            list_selected: Signal::new(rt.clone(), None),
            list_offset: Signal::new(rt, 0.0),
        }
    });
}
