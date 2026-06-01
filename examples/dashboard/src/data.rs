//! Synthetic process/resource fixtures for the dashboard. No OS enumeration —
//! static data is enough to exercise every widget (per the program's scope).

use slate_framework::{Column, TreeNode};

/// Flat process names for the sidebar [`VirtualList`](slate_framework::VirtualList).
pub const PROCESSES: &[&str] = &[
    "kernel_task",
    "WindowServer",
    "launchd",
    "slate-framework",
    "Terminal",
    "Safari",
    "mds_stores",
    "cfprefsd",
];

/// Options for the "Sort by" [`Select`](slate_framework::Select).
pub const SORT_FIELDS: &[&str] = &["PID", "Process", "CPU %", "Mem MB"];

/// Process hierarchy for the sidebar [`Tree`](slate_framework::Tree). Ids are
/// stable + unique (the Tree uses them as expansion/selection keys).
pub fn process_tree() -> Vec<TreeNode> {
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

/// Columns of the main process [`DataGrid`](slate_framework::DataGrid).
pub fn grid_columns() -> Vec<Column> {
    vec![
        Column::new("PID", 70.0),
        Column::new("Process", 220.0),
        Column::new("CPU %", 90.0),
        Column::new("Mem MB", 100.0),
    ]
}

/// 60 synthetic process rows — enough to exercise scroll + virtualization.
pub fn grid_rows() -> Vec<Vec<String>> {
    (0..60)
        .map(|i| {
            let name = PROCESSES[i % PROCESSES.len()];
            vec![
                format!("{}", 100 + i * 7),
                name.to_string(),
                format!("{:.1}", (i as f32 * 3.7) % 40.0),
                format!("{}", 32 + (i * 53) % 1024),
            ]
        })
        .collect()
}

/// Per-core load percentages for the [`BarChart`](slate_framework::BarChart).
pub fn per_core_load() -> Vec<f32> {
    (0..8).map(|i| 20.0 + (i as f32 * 11.0) % 70.0).collect()
}

/// CPU-history series for the [`Sparkline`](slate_framework::Sparkline).
pub fn cpu_history() -> Vec<f32> {
    (0..32).map(|i| 30.0 + 25.0 * ((i as f32) * 0.5).sin()).collect()
}
