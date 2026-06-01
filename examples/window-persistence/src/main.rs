//! window-persistence — P6d window geometry persistence demo.
//!
//! Shows the pluggable `PersistenceStore`: a tiny adopter-owned file store
//! saves the window's position/size/maximized state, and the framework restores
//! it on the next launch. The framework owns the `WindowGeometry` struct and the
//! save/restore wiring; this example owns the storage format + location.
//!
//! Manual check (macOS): run, move/resize the window, quit, run again — the
//! window reappears where you left it.
//!
//! Run: `cargo run -p window-persistence`

use std::path::PathBuf;
use std::rc::Rc;

use slate_framework::{
    AlignItems, AnyElement, App, AppContext, Color, Div, FlexDirection, IntoAny, JustifyContent,
    PersistenceStore, Text, View, WindowGeometry, WindowOptions,
};

/// A minimal plain-text geometry store: one space-separated line
/// `x y w h maximized`. `x = i32::MIN` encodes an unknown position. No serde —
/// the framework leaves serialization entirely to the adopter.
struct FileStore {
    path: PathBuf,
}

impl PersistenceStore for FileStore {
    fn load(&self, _key: &str) -> Option<WindowGeometry> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        let mut it = text.split_whitespace();
        let x: i32 = it.next()?.parse().ok()?;
        let y: i32 = it.next()?.parse().ok()?;
        let w: u32 = it.next()?.parse().ok()?;
        let h: u32 = it.next()?.parse().ok()?;
        let maximized = it.next()? != "0";
        Some(WindowGeometry {
            position: (x != i32::MIN).then_some((x, y)),
            size: (w, h),
            maximized,
        })
    }

    fn save(&self, _key: &str, geometry: WindowGeometry) {
        let (x, y) = geometry.position.unwrap_or((i32::MIN, i32::MIN));
        let line = format!(
            "{x} {y} {} {} {}",
            geometry.size.0, geometry.size.1, geometry.maximized as u8
        );
        if let Err(e) = std::fs::write(&self.path, line) {
            log::warn!("failed to save window geometry: {e}");
        }
    }
}

struct PersistenceView;

impl View for PersistenceView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        Div::new()
            .background(Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK))
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .align_items(AlignItems::Center)
                    .justify_content(JustifyContent::Center)
                    .gap(12.0)
                    .padding_all(32.0)
                    .flex_grow(1.0)
            })
            .child(
                Text::new("Window Persistence")
                    .font_size(24.0)
                    .color(Color::WHITE.into()),
            )
            .child(
                Text::new("Move / resize this window, quit, then run again.")
                    .font_size(14.0)
                    .color(Color::from_hex("#94a3b8").unwrap_or(Color::WHITE).into()),
            )
            .child(
                Text::new("It reopens where you left it.")
                    .font_size(14.0)
                    .color(Color::from_hex("#94a3b8").unwrap_or(Color::WHITE).into()),
            )
            .into_any()
    }
}

fn main() {
    env_logger::init();

    let path = std::env::temp_dir().join("slate-window-persistence.geom");
    log::info!("geometry stored at: {}", path.display());
    let store = Rc::new(FileStore { path });

    App::new(WindowOptions {
        title: "Slate · window-persistence".into(),
        size: (640, 480),
        min_size: Some((320, 240)),
        resizable: true,
        persistence_key: Some("main".into()),
        ..Default::default()
    })
    .persistence(store)
    .run(|_cx: &AppContext| PersistenceView);
}
