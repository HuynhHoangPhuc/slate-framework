//! ScrollView virtualization regression tests (uniform-height windowing).
//!
//! Proves the headline virtualization property: only the visible window of rows
//! is ever materialized/painted, regardless of total `count`; the window slides
//! with the scroll offset; and rows land at their arithmetic content position.
//! The pure window math lives in `scroll_view::virtual_list` unit tests; these
//! drive the wired element through the headless paint pipeline.
//!
//! Each row encodes its absolute index in its **width** (`ROW_W_BASE + i`) so a
//! painted rect can be mapped straight back to the logical row it came from.

use slate_framework::reactive::Signal;
use slate_framework::{AnyElement, Color, Div, HeadlessApp, IntoAny, ScrollView, View};

const VIEWPORT_W: f32 = 400.0;
const VIEWPORT_H: f32 = 200.0;
const ROW_H: f32 = 20.0;
const ROW_W_BASE: f32 = 10.0;
const COUNT: usize = 1000;

/// Build a virtualized list of `COUNT` rows; row `i` is `ROW_W_BASE + i` wide.
struct VirtualDemo {
    offset: Signal<f32>,
}

impl View for VirtualDemo {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        ScrollView::new()
            .offset(self.offset.clone())
            .style(|s| s.width(VIEWPORT_W).height(VIEWPORT_H).column())
            .virtualized(COUNT, ROW_H, |i| {
                Div::new()
                    .background(Color::RED)
                    .style(move |s| s.width(ROW_W_BASE + i as f32).height(ROW_H))
            })
            .into_any()
    }
}

/// Absolute row indices recovered from painted row rects (width → index).
/// Excludes the scrollbar thumb (an 8px-wide rect, narrower than any row).
fn painted_row_indices(app: &HeadlessApp) -> Vec<usize> {
    let mut out: Vec<usize> = app
        .scene()
        .rects
        .iter()
        .filter(|r| r.rect[2].0 >= ROW_W_BASE)
        .map(|r| (r.rect[2].0 - ROW_W_BASE).round() as usize)
        .collect();
    out.sort_unstable();
    out
}

#[test]
fn only_visible_window_is_materialized_at_top() {
    let mut app = HeadlessApp::with_scale_factor(VIEWPORT_W as u32, VIEWPORT_H as u32, 1.0)
        .expect("app");
    let offset = Signal::new(app.runtime(), 0.0);
    let mut view = VirtualDemo {
        offset: offset.clone(),
    };
    let _ = app.render_view(&mut view).expect("render");

    let rows = painted_row_indices(&app);
    // 200px viewport / 20px rows = 10 visible + 1 partial + 2 overscan = 13.
    // The window clamps at the top so it starts at row 0, not below it.
    assert_eq!(rows, (0..13).collect::<Vec<_>>(), "window = rows 0..13");
    // The whole point: a 1000-row list paints ~13 rows, not 1000.
    assert!(rows.len() < 20, "virtualized count stays tiny: {}", rows.len());
}

#[test]
fn window_slides_with_offset_and_drops_offscreen_rows() {
    let mut app = HeadlessApp::with_scale_factor(VIEWPORT_W as u32, VIEWPORT_H as u32, 1.0)
        .expect("app");
    let offset = Signal::new(app.runtime(), 0.0);
    let mut view = VirtualDemo {
        offset: offset.clone(),
    };
    let _ = app.render_view(&mut view).expect("first render");
    assert!(painted_row_indices(&app).contains(&0), "row 0 visible at top");

    // Scroll down 5000px = 250 rows. first_visible 250, overscan 2 → 248.
    offset.set(5000.0);
    let _ = app.render_view(&mut view).expect("scrolled render");

    let rows = painted_row_indices(&app);
    assert_eq!(*rows.first().unwrap(), 248, "window starts at 248 (250 - overscan)");
    assert!(!rows.contains(&0), "row 0 is no longer materialized");
    assert!(rows.contains(&250), "row 250 is now in view");
    assert!(rows.len() < 20, "window still tiny after scroll: {}", rows.len());
}

#[test]
fn rows_land_at_arithmetic_content_position() {
    let mut app = HeadlessApp::with_scale_factor(VIEWPORT_W as u32, VIEWPORT_H as u32, 1.0)
        .expect("app");
    let offset = Signal::new(app.runtime(), 0.0);
    let mut view = VirtualDemo {
        offset: offset.clone(),
    };

    // Scroll so row 250's top aligns exactly to the viewport top:
    // content_y(250) = 250 * 20 = 5000; screen_y = 5000 - offset = 0.
    offset.set(5000.0);
    let _ = app.render_view(&mut view).expect("render");

    // Find row 250's rect (width = ROW_W_BASE + 250 = 260) and assert y ≈ 0.
    let row_250 = app
        .scene()
        .rects
        .iter()
        .find(|r| (r.rect[2].0 - (ROW_W_BASE + 250.0)).abs() < 0.5)
        .expect("row 250 painted");
    assert!(row_250.rect[1].0.abs() < 0.5, "row 250 at y≈0, got {}", row_250.rect[1].0);
}

#[test]
fn auto_height_falls_back_to_rendering_all_rows() {
    // No fixed pixel height → the window can't be sized, so every row renders.
    // (Kept tiny; correctness of the fallback, not its cost.)
    const SMALL: usize = 5;
    struct AutoView {
        offset: Signal<f32>,
    }
    impl View for AutoView {
        fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
            ScrollView::new()
                .offset(self.offset.clone())
                .style(|s| s.width(VIEWPORT_W).column())
                .virtualized(SMALL, ROW_H, |i| {
                    Div::new()
                        .background(Color::GREEN)
                        .style(move |s| s.width(ROW_W_BASE + i as f32).height(ROW_H))
                })
                .into_any()
        }
    }
    let mut app =
        HeadlessApp::with_scale_factor(VIEWPORT_W as u32, VIEWPORT_H as u32, 1.0).expect("app");
    let mut view = AutoView {
        offset: Signal::new(app.runtime(), 0.0),
    };
    let _ = app.render_view(&mut view).expect("render");

    assert_eq!(
        painted_row_indices(&app),
        (0..SMALL).collect::<Vec<_>>(),
        "all rows render when height is not a fixed pixel value"
    );
}
