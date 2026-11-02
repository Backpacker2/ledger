//! The parts of the screenshot feature that don't need a screen: which shortcuts are accepted, how a dragged
//! region is cropped, and what is sent to (and understood from) the upload endpoint.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

use image::{Rgba, RgbaImage};

use crate::screenshot::{DEFAULT_HOTKEY, Region, crop, parse_hotkey, post_image};

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

/// A one-shot HTTP server: answers the first request with `reply` and hands the request back.
fn serve(status: &str, content_type: &str, reply: &str) -> (String, mpsc::Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/upload", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let n = stream.read(&mut buf).unwrap();
            request.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&request).to_string();
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length: ").and_then(|v| v.trim().parse::<usize>().ok())).unwrap_or(0);
                if request.len() >= end + 4 + length {
                    break;
                }
            }
            if n == 0 {
                break;
            }
        }
        stream.write_all(response.as_bytes()).unwrap();
        tx.send(request).unwrap();
    });
    (url, rx)
}

#[test]
fn the_image_is_posted_as_a_png_in_a_multipart_file_field() {
    let (url, request) = serve("200 OK", "application/json", r#"{"url":"https://img.example/abc.png"}"#);
    let uploaded = post_image(&url, &picture()).unwrap();
    assert_eq!((uploaded.url.as_str(), uploaded.width, uploaded.height), ("https://img.example/abc.png", 8, 6));

    let request = request.recv().unwrap();
    let text = String::from_utf8_lossy(&request);
    assert!(text.starts_with("POST /upload "), "{text}");
    assert!(text.to_lowercase().contains("content-type: multipart/form-data; boundary="));
    assert!(text.contains(r#"name="file"; filename="screenshot-"#), "{text}");
    assert!(text.contains("Content-Type: image/png"));
    assert!(request.windows(4).any(|w| w == [0x89, b'P', b'N', b'G']), "the body holds a real PNG");
}

#[test]
fn the_endpoints_answer_is_understood_in_its_common_shapes() {
    for (content_type, reply) in [
        ("application/json", r#"{"link":"https://x.example/1.png"}"#),
        ("application/json", r#"{"data":{"url":"https://x.example/1.png"}}"#),
        ("text/plain", "https://x.example/1.png\n"),
    ] {
        let (url, _) = serve("200 OK", content_type, reply);
        assert_eq!(post_image(&url, &picture()).unwrap().url, "https://x.example/1.png", "{reply}");
    }
}

#[test]
fn upload_failures_say_what_went_wrong() {
    let (url, _) = serve("500 Internal Server Error", "text/plain", "boom");
    assert!(post_image(&url, &picture()).unwrap_err().to_string().contains("500"));

    let (url, _) = serve("200 OK", "application/json", r#"{"ok":true}"#);
    assert!(post_image(&url, &picture()).unwrap_err().to_string().contains("didn't contain the image's link"));

    // Nothing listening there at all.
    let message = post_image("http://127.0.0.1:1/upload", &picture()).unwrap_err().to_string();
    assert!(message.contains("Could not reach the upload endpoint"), "{message}");
}
