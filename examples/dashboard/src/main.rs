//! dashboard — the P5 reference app: a process/resource monitor assembled
//! entirely from the extracted widget set.
//!
//! Layout: a [`Toolbar`](slate_framework::Toolbar) on top, a
//! [`Splitter`](slate_framework::Splitter) dividing a sidebar
//! ([`Tree`](slate_framework::Tree) + [`VirtualList`](slate_framework::VirtualList))
//! from the main area (a virtualized [`DataGrid`](slate_framework::DataGrid),
//! primitive-drawn [`BarChart`](slate_framework::BarChart) +
//! [`Sparkline`](slate_framework::Sparkline) viz, a settings strip of every
//! form control + a [`Select`](slate_framework::Select), and a right-click
//! [`ContextMenu`](slate_framework::ContextMenu)), over a
//! [`StatusBar`](slate_framework::StatusBar). The top-right button flips
//! light/dark. All data is synthetic (no OS enumeration). Panel builders live in
//! [`panels`]; fixtures in [`data`].
//!
//! Run: `cargo run -p dashboard`

mod data;
mod panels;

use std::collections::HashSet;
use std::time::Instant;

use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, App, AppContext, Bounds, Div, FlexDirection, IntoAny, Splitter,
    ThemeMode, View, WindowOptions, theme,
};

/// Caller-owned shell state, created once in `App::run` so it survives the
/// Strategy-A whole-view rebuild.
pub struct DashboardView {
    pub mode: Signal<ThemeMode>,
    /// Sidebar/main split ratio (first-pane fraction).
    pub split: Signal<f32>,
    pub tree_expanded: Signal<HashSet<u64>>,
    pub tree_selected: Signal<Option<u64>>,
    pub list_selected: Signal<Option<usize>>,
    pub list_offset: Signal<f32>,
    pub grid_active: Signal<(usize, usize)>,
    pub grid_offset: Signal<f32>,
    pub grid_selected: Signal<Option<usize>>,
    pub sort_by: Signal<usize>,
    pub sort_open: Signal<bool>,
    pub sort_anchor: Signal<Bounds>,
    pub sort_scroll: Signal<f32>,
    pub show_system: Signal<bool>,
    pub auto_refresh: Signal<bool>,
    pub refresh_secs: Signal<f32>,
    pub filter_text: Signal<String>,
    pub tip_hover: Signal<Option<Instant>>,
    pub ctx_open: Signal<bool>,
    pub ctx_anchor: Signal<Bounds>,
    pub last_action: Signal<String>,
}

impl View for DashboardView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let t = theme();
        let splitter = Splitter::new(self.split.clone())
            .first(self.sidebar())
            .second(self.main_pane())
            .min_sizes(160.0, 360.0)
            .label("Sidebar / main divider");

        Div::new()
            .background(t.bg)
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .align_items(AlignItems::Stretch)
                    .flex_grow(1.0)
            })
            .child(self.toolbar())
            .child(splitter)
            .child(self.status())
            .into_any()
    }
}

fn main() {
    env_logger::init();

    App::new(WindowOptions {
        title: "Slate · dashboard".into(),
        size: (1040, 720),
        min_size: Some((720, 520)),
        resizable: true,
        ..Default::default()
    })
    .run(|cx: &AppContext| {
        let rt = cx.runtime();
        let mode = cx.theme_mode().expect("theme_mode available inside App::run");
        DashboardView {
            mode,
            split: Signal::new(rt.clone(), 0.26),
            tree_expanded: Signal::new(rt.clone(), HashSet::from([0, 10])),
            tree_selected: Signal::new(rt.clone(), None),
            list_selected: Signal::new(rt.clone(), None),
            list_offset: Signal::new(rt.clone(), 0.0),
            grid_active: Signal::new(rt.clone(), (0, 0)),
            grid_offset: Signal::new(rt.clone(), 0.0),
            grid_selected: Signal::new(rt.clone(), None),
            sort_by: Signal::new(rt.clone(), 0),
            sort_open: Signal::new(rt.clone(), false),
            sort_anchor: Signal::new(rt.clone(), Bounds::ZERO),
            sort_scroll: Signal::new(rt.clone(), 0.0),
            show_system: Signal::new(rt.clone(), true),
            auto_refresh: Signal::new(rt.clone(), false),
            refresh_secs: Signal::new(rt.clone(), 3.0),
            filter_text: Signal::new(rt.clone(), String::new()),
            tip_hover: Signal::new(rt.clone(), None),
            ctx_open: Signal::new(rt.clone(), false),
            ctx_anchor: Signal::new(rt.clone(), Bounds::ZERO),
            last_action: Signal::new(rt, "Ready".to_string()),
        }
    });
}
