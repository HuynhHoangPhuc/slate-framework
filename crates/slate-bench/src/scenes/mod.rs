//! Reference scenes. Each module exposes a `build(app)` constructor that
//! returns a `View` ready to be driven through `HeadlessApp::render_view`.

pub mod dense_image_gallery;
pub mod fine_grained_subscription;
pub mod ime_textfield;
pub mod large_scroll_list;
pub mod reactive_counter_100;
pub mod textarea_1k_line;
