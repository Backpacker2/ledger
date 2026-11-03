//! The parts of the screenshot feature that don't need a screen: which shortcuts are accepted, how the user's own
//! tool is run and understood, which files are taken as pictures, and how a dragged region is cropped.

use std::io::Cursor;
use std::path::Path;

use image::{ImageFormat, Rgba, RgbaImage};

use crate::screenshot::command::{Outcome, interpret, prepare, split};
use crate::screenshot::{DEFAULT_HOTKEY, Region, crop, dimensions, file_name, is_png_name, parse_hotkey, save_tool};
use crate::settings::{ScreenshotTool, read_settings};

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

// ---- the user's own tool ----------------------------------------------------------------------------

#[test]
fn a_command_splits_on_spaces_and_quotes_keep_a_part_together() {
    assert_eq!(split("flameshot gui --raw").unwrap(), ["flameshot", "gui", "--raw"]);
    assert_eq!(split("  screencapture   -i {file} ").unwrap(), ["screencapture", "-i", "{file}"]);
    // Windows paths: the quotes go, the backslashes stay.
    assert_eq!(
        split(r#""C:\Program Files\Flameshot\bin\flameshot.exe" gui -p {file}"#).unwrap(),
        [r"C:\Program Files\Flameshot\bin\flameshot.exe", "gui", "-p", "{file}"]
    );
    assert_eq!(split("tool 'two words' a\"b\"c \"\"").unwrap(), ["tool", "two words", "abc", ""]);
}

#[test]
fn a_command_with_an_open_quote_or_nothing_in_it_is_refused() {
    assert!(split("flameshot \"gui").unwrap_err().to_string().contains("quote"));
    for empty in ["", "   "] {
        assert!(split(empty).is_err(), "{empty:?}");
    }
}

#[test]
fn file_is_filled_in_with_the_temporary_path_wherever_it_appears() {
    let (parts, uses_file) = prepare("screencapture -i {file}", "/tmp/a b.png").unwrap();
    assert_eq!(parts, ["screencapture", "-i", "/tmp/a b.png"]);
    assert!(uses_file);

    let (parts, _) = prepare("tool --out={file}", "/tmp/x.png").unwrap();
    assert_eq!(parts, ["tool", "--out=/tmp/x.png"]);

    let (parts, uses_file) = prepare("flameshot gui --raw", "/tmp/x.png").unwrap();
    assert_eq!(parts, ["flameshot", "gui", "--raw"]);
    assert!(!uses_file);
}

#[test]
fn a_picture_is_taken_whatever_the_exit_and_no_picture_is_a_cancel_unless_the_tool_complains() {
    assert_eq!(interpret(true, vec![1, 2], "").unwrap(), Outcome::Picture(vec![1, 2]));
    assert_eq!(interpret(false, vec![1, 2], "warning").unwrap(), Outcome::Picture(vec![1, 2]));

    // Esc: macOS's screencapture exits cleanly with nothing; Flameshot says so and exits with 1.
    assert_eq!(interpret(true, vec![], "").unwrap(), Outcome::Cancelled);
    assert_eq!(interpret(false, vec![], "flameshot: info: Screenshot aborted.\n").unwrap(), Outcome::Cancelled);
    assert_eq!(interpret(false, vec![], "Capture cancelled by user").unwrap(), Outcome::Cancelled);
    assert_eq!(interpret(false, vec![], "  ").unwrap(), Outcome::Cancelled);

    // A real failure is shown, its last lines only.
    let said = "line 1\nline 2\nUnknown option 'x'\nUsage: flameshot gui [options]";
    let message = interpret(false, vec![], said).unwrap_err().to_string();
    assert!(message.contains("Unknown option 'x' Usage: flameshot gui [options]"), "{message}");
    assert!(!message.contains("line 1"), "{message}");
}

#[test]
fn the_tool_choice_is_saved_and_a_broken_command_is_not() {
    let vault = tempfile::tempdir().unwrap();
    let root = vault.path();
    assert_eq!(read_settings(root).screenshot_tool, ScreenshotTool::default());

    save_tool(root, true, "  flameshot gui --raw ").unwrap();
    assert_eq!(read_settings(root).screenshot_tool, ScreenshotTool { use_command: true, command: "flameshot gui --raw".into() });

    // Back to the built-in capture: the command is remembered for later.
    save_tool(root, false, "flameshot gui --raw").unwrap();
    assert_eq!(read_settings(root).screenshot_tool, ScreenshotTool { use_command: false, command: "flameshot gui --raw".into() });

    assert!(save_tool(root, true, "").is_err());
    assert!(save_tool(root, true, "tool \"open").is_err());
    assert!(!read_settings(root).screenshot_tool.command.contains("open"));
    // Unused, a half-typed command is fine.
    save_tool(root, false, "tool \"open").unwrap();
}

// ---- pictures handed over ---------------------------------------------------------------------------

#[test]
fn only_png_files_are_taken_as_screenshots() {
    for yes in ["/tmp/flameshot-1.png", "shot.PNG", "C:\\x\\y.Png"] {
        assert!(is_png_name(Path::new(yes)), "{yes}");
    }
    for no in ["/tmp/notes.md", "--flag", "png", "/tmp/x.png.txt", "/tmp/photo.jpg"] {
        assert!(!is_png_name(Path::new(no)), "{no}");
    }
}

#[test]
fn a_png_is_measured_from_its_header_and_anything_else_is_refused() {
    let mut png = Vec::new();
    picture().write_to(&mut Cursor::new(&mut png), ImageFormat::Png).unwrap();
    assert_eq!(dimensions(&png).unwrap(), (8, 6));

    assert!(dimensions(b"not a picture").is_err());
    assert!(dimensions(&[]).is_err());
    assert!(dimensions(&png[..12]).is_err());
}

#[cfg(unix)]
#[test]
fn a_real_tool_hands_its_picture_back_through_the_file_or_what_it_prints() {
    use crate::screenshot::command::run;

    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("shot 1.png");
    picture().save_with_format(&source, ImageFormat::Png).unwrap();
    let png = std::fs::read(&source).unwrap();
    let quoted = format!("\"{}\"", source.display());

    // Written to {file}: read, then the temporary file is gone.
    assert_eq!(run(&format!("cp {quoted} {{file}}")).unwrap(), Outcome::Picture(png.clone()));
    // Printed.
    assert_eq!(run(&format!("cat {quoted}")).unwrap(), Outcome::Picture(png));
    // Exits without a picture: cancelled.
    assert_eq!(run("true").unwrap(), Outcome::Cancelled);
    assert_eq!(run("sh -c 'echo Screenshot aborted. >&2; exit 1'").unwrap(), Outcome::Cancelled);
    // Complains: an error with its words.
    assert!(run("sh -c 'echo bad flag >&2; exit 2'").unwrap_err().to_string().contains("bad flag"));
    // Isn't there.
    assert!(run("no-such-screenshot-tool-here").unwrap_err().to_string().contains("couldn't start no-such-screenshot-tool-here"));
}
