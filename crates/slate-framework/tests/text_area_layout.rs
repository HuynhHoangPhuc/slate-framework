//! Integration smoke test for the multi-line `TextArea` element.
//!
//! There is no cross-platform render harness (`test_support.rs` covers GPU
//! recovery only), so `prepaint`/`paint` cannot run here — the layout caching
//! onto `ImeState` happens inside `paint` and is exercised manually by the
//! framework at runtime. What this test *can* verify without a GPU is the two
//! halves TextArea is built from:
//!
//!   1. the element's public API constructs (builder + style chain), and
//!   2. the real Windows DirectWrite layout path produces the expected
//!      visual-line model — a 3-row hard-newline document wraps to 3 lines and
//!      the document-start caret resolves to line 0.
//!
//! Runs cross-platform as a `harness = false` `[[test]]` (its `fn main` executes
//! on the process main thread); this also exercises the CoreText layout path on
//! macOS, not just DirectWrite. `required-features = ["test-hooks"]` gates it.

use slate_framework::elements::{TextArea, TextAreaStyle};
use slate_framework::text_system::TextSystem;
use slate_reactive::{Runtime, Signal};

/// `assert_eq!` analogue for the `harness = false` runner: returns `Err` with
/// both operands on mismatch instead of panicking.
macro_rules! ensure_eq {
    ($left:expr, $right:expr, $($arg:tt)*) => {{
        let left = $left;
        let right = $right;
        if left != right {
            return Err(format!(
                "{} (left: {:?}, right: {:?})",
                format!($($arg)*),
                left,
                right
            ));
        }
    }};
}

/// `assert!` analogue: returns `Err` when the condition is false. Uses a `match`
/// on the boolean rather than `if !cond` so a float comparison like `x > y`
/// doesn't trip the negated-partial-ord lint.
macro_rules! ensure {
    ($cond:expr, $($arg:tt)*) => {
        match $cond {
            true => {}
            false => return Err(format!($($arg)*)),
        }
    };
}

/// One named runner case: a label and the check fn that returns `Err` on failure.
type Case = (&'static str, fn() -> Result<(), String>);

// ---------------------------------------------------------------------------
// Smoke: TextArea public API compiles (no view tree required)
// ---------------------------------------------------------------------------

fn check_text_area_module_smoke() -> Result<(), String> {
    let rt = Runtime::new();
    let signal = Signal::new(rt, String::new());
    let style = TextAreaStyle {
        width: 320.0,
        min_lines: 3,
        ..Default::default()
    };
    // Builder chain + custom style compile and bind to the signal.
    let _ta = TextArea::new(signal).style(style);
    Ok(())
}

// ---------------------------------------------------------------------------
// Layout: hard newlines wrap to one visual line each; caret-at-start → line 0
// ---------------------------------------------------------------------------

fn check_three_line_document_wraps_to_three_lines_with_caret_on_line_zero() -> Result<(), String> {
    // Real platform text system — shaping only, no GPU device required.
    let mut text_system = TextSystem::new().expect("create TextSystem");
    let font = text_system
        .load_font_from_bytes(slate_text::TEST_FONT, 14.0, 1.0)
        .expect("load bundled font");

    // Three hard-newline-separated paragraphs, each short enough to occupy a
    // single visual line at a generous width.
    let doc = text_system
        .shape_document(&font, "alpha\nbeta\ngamma")
        .expect("shape document");
    let layout = slate_text::wrap_document(&doc, 1000.0);

    ensure_eq!(
        layout.lines.len(),
        3,
        "two hard newlines must yield three visual lines"
    );

    // Byte ranges tile 0..text.len() with no gaps.
    ensure_eq!(layout.lines[0].byte_start, 0, "first line starts at byte 0");
    for pair in layout.lines.windows(2) {
        ensure_eq!(
            pair[0].byte_end, pair[1].byte_start,
            "visual-line byte ranges must be contiguous"
        );
    }
    ensure_eq!(
        layout.lines.last().unwrap().byte_end,
        "alpha\nbeta\ngamma".len(),
        "final line must cover to text end"
    );

    // The document-start caret (focused-field initial position) lands on line 0
    // at the line's left edge.
    let (line_idx, x_lpx, y_lpx) = layout.caret_position(0);
    ensure_eq!(line_idx, 0, "caret at byte 0 resolves to the first line");
    ensure_eq!(x_lpx, 0.0, "caret at byte 0 sits at the line's left edge");
    ensure_eq!(y_lpx, 0.0, "first line's top is at y = 0");

    // Vertical layout is monotonic top-to-bottom by a uniform line height.
    ensure!(layout.line_height_lpx > 0.0, "line height is positive");
    ensure_eq!(
        layout.lines[1].line.y_offset_lpx,
        layout.line_height_lpx,
        "line1 sits one line-height down"
    );
    ensure_eq!(
        layout.lines[2].line.y_offset_lpx,
        2.0 * layout.line_height_lpx,
        "line2 sits two line-heights down"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Multi-space: the word-wrap path (TextArea) preserves every ASCII space, so
// caret-x advances once per space byte instead of collapsing the run.
// ---------------------------------------------------------------------------

fn check_multi_space_run_preserves_every_space_byte() -> Result<(), String> {
    let mut text_system = TextSystem::new().expect("create TextSystem");
    let font = text_system
        .load_font_from_bytes(slate_text::TEST_FONT, 14.0, 1.0)
        .expect("load bundled font");

    // Single space vs five spaces between the same two words: the 5-space run
    // must be ~4 extra space-advances wider (not collapsed to one).
    let one = text_system.shape_document(&font, "a b").expect("shape one");
    let five = text_system.shape_document(&font, "a     b").expect("shape five");
    let one_w = slate_text::wrap_document(&one, 1000.0).lines[0].line.width_lpx;
    let five_w = slate_text::wrap_document(&five, 1000.0).lines[0].line.width_lpx;
    ensure!(
        five_w > one_w + 1.0,
        "5-space run must be wider than 1-space ({five_w} vs {one_w})"
    );

    // Caret-x is strictly increasing across each space byte of "a     b"
    // (bytes 1..=6 are the run interior + the following 'b').
    let layout = slate_text::wrap_document(&five, 1000.0);
    let mut prev_x = layout.caret_position(1).1; // just after 'a'
    for byte in 2..=6 {
        let x = layout.caret_position(byte).1;
        ensure!(
            x > prev_x,
            "caret-x must advance at space byte {byte}: {x} !> {prev_x}"
        );
        prev_x = x;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Regression guard for the TextField path: TextField shapes its whole display
// string via `shape_line` (not the word-wrap path), which already preserves
// spaces. Lock that in so the TextArea fix never tempts a shared regression.
// ---------------------------------------------------------------------------

fn check_text_field_shape_line_preserves_spaces() -> Result<(), String> {
    let mut text_system = TextSystem::new().expect("create TextSystem");
    let font = text_system
        .load_font_from_bytes(slate_text::TEST_FONT, 14.0, 1.0)
        .expect("load bundled font");

    let one = text_system.shape_line(&font, "a b").expect("shape one");
    let five = text_system.shape_line(&font, "a     b").expect("shape five");
    ensure!(
        five.width_lpx > one.width_lpx + 1.0,
        "shape_line must keep all 5 spaces ({} vs {})",
        five.width_lpx,
        one.width_lpx
    );
    Ok(())
}

fn main() {
    let cases: &[Case] = &[
        ("text_area_module_smoke", check_text_area_module_smoke),
        (
            "three_line_document_wraps_to_three_lines_with_caret_on_line_zero",
            check_three_line_document_wraps_to_three_lines_with_caret_on_line_zero,
        ),
        (
            "multi_space_run_preserves_every_space_byte",
            check_multi_space_run_preserves_every_space_byte,
        ),
        (
            "text_field_shape_line_preserves_spaces",
            check_text_field_shape_line_preserves_spaces,
        ),
    ];

    let mut failed = 0;
    for (name, f) in cases {
        match f() {
            Ok(()) => println!("ok   - {name}"),
            Err(e) => {
                eprintln!("FAIL - {name}: {e}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        eprintln!("\n{failed} case(s) failed");
        std::process::exit(1);
    }
    println!("\nall {} case(s) passed", cases.len());
}
