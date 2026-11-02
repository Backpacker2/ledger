//! The parts of the screenshot feature that don't need a screen: which shortcuts are accepted, and how a dragged
//! region is cropped.

use image::{Rgba, RgbaImage};

use crate::screenshot::{DEFAULT_HOTKEY, Region, crop, file_name, parse_hotkey};

#[test]
fn the_default_and_ordinary_shortcuts_are_accepted() {
    for ok in [DEFAULT_HOTKEY, "Control+Shift+S", "Alt+F9", "Super+Shift+4", "Control+Alt+ArrowUp", "F9", "PrintScreen", "Shift+PrintScreen"] {
        assert!(parse_hotkey(ok).is_ok(), "{ok}");
    }
}

#[test]
fn shortcuts_that_would_take_over_a_key_or_clash_with_the_system_are_refused() {
    // No modifier: it would swallow that key in every program.
    for bad in ["S", "Space", "Enter", "Shift+"] {
        assert!(parse_hotkey(bad).is_err(), "{bad}");
    }
    // Reserved, and gibberish.
    for reserved in ["Alt+F4", "alt+tab", "Control+C", "Control+Alt+Delete", "Command+Q"] {
        let message = parse_hotkey(reserved).unwrap_err().to_string();
        assert!(message.contains("already use"), "{reserved}: {message}");
    }
    for nonsense in ["", "Control+Shift+Banana", "Control+A+B", "+S"] {
        assert!(parse_hotkey(nonsense).is_err(), "{nonsense:?}");
    }
}

fn picture() -> RgbaImage {
    // 8 × 6; the pixel at (x, y) is (x, y, 0, 255) so a crop can be checked by looking at its first pixel.
    RgbaImage::from_fn(8, 6, |x, y| Rgba([x as u8, y as u8, 0, 255]))
}

#[test]
fn a_region_is_cropped_out_of_the_capture_and_clamped_to_it() {
    let r = |x, y, width, height| Some(Region { x, y, width, height });

    let cropped = crop(picture(), r(2, 1, 3, 4)).unwrap();
    assert_eq!(cropped.dimensions(), (3, 4));
    assert_eq!(cropped.get_pixel(0, 0), &Rgba([2, 1, 0, 255]));

    // A rectangle that runs past the edge is cut at the edge rather than refused: the overlay's rounding can be off by a pixel.
    let cut = crop(picture(), r(6, 4, 10, 10)).unwrap();
    assert_eq!(cut.dimensions(), (2, 2));

    // No region means the whole picture (what the Wayland portal hands back is already the chosen area).
    assert_eq!(crop(picture(), None).unwrap().dimensions(), (8, 6));

    for empty in [r(0, 0, 0, 3), r(0, 0, 3, 0), r(8, 0, 2, 2), r(0, 6, 2, 2)] {
        assert!(crop(picture(), empty).is_err());
    }
}

#[test]
fn screenshots_are_saved_under_a_dated_png_name() {
    let name = file_name();
    assert!(name.starts_with("screenshot-") && name.ends_with(".png"), "{name}");
    assert_eq!(name.len(), "screenshot-20260101-120000.png".len());
}
