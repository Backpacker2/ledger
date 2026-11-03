//! The app's log: entries are kept in order, a long message is cut off, the files stay within the limit, and
//! clearing it empties both files.

use std::fs;
use std::sync::{Arc, Mutex};

use crate::applog::{DEFAULT_MAX_BYTES, FILE_NAME, Log, MAX_MAX_BYTES, MIN_MAX_BYTES, OLD_FILE_NAME};

#[test]
fn entries_come_back_oldest_first_and_limited() {
    let dir = tempfile::tempdir().unwrap();
    let log = Log::new(dir.path(), DEFAULT_MAX_BYTES, None);
    log.write("error", "sync", "first");
    log.write("info", "app", "second");
    log.write("warn", "trello", "third");

    let all = log.read(10);
    assert_eq!(all.iter().map(|e| e.message.as_str()).collect::<Vec<_>>(), ["first", "second", "third"]);
    assert_eq!(all[0].level, "error");
    assert_eq!(all[0].source, "sync");

    let last = log.read(2);
    assert_eq!(last.iter().map(|e| e.message.as_str()).collect::<Vec<_>>(), ["second", "third"]);
}

#[test]
fn an_unknown_level_is_info_and_a_long_message_is_cut_off() {
    let dir = tempfile::tempdir().unwrap();
    let log = Log::new(dir.path(), DEFAULT_MAX_BYTES, None);
    log.write("shout", "ui", &"é".repeat(10_000));
    let entry = &log.read(1)[0];
    assert_eq!(entry.level, "info");
    assert!(entry.message.len() < 9_000);
    assert!(entry.message.ends_with('…'));
}

#[test]
fn a_broken_line_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let log = Log::new(dir.path(), DEFAULT_MAX_BYTES, None);
    log.write("error", "a", "kept");
    let file = dir.path().join(FILE_NAME);
    let mut text = fs::read_to_string(&file).unwrap();
    text.push_str("{\"at\": \"half a li\n");
    fs::write(&file, text).unwrap();
    log.write("error", "b", "also kept");
    assert_eq!(log.read(10).len(), 2);
}

#[test]
fn a_big_log_moves_aside_and_is_still_read() {
    let dir = tempfile::tempdir().unwrap();
    let log = Log::new(dir.path(), DEFAULT_MAX_BYTES, None);
    fs::write(dir.path().join(FILE_NAME), "x".repeat(1024 * 1024 + 1)).unwrap();
    log.write("info", "app", "after the move");
    assert!(dir.path().join(OLD_FILE_NAME).exists());
    assert_eq!(log.read(10).last().unwrap().message, "after the move");
}

#[test]
fn clearing_empties_the_log_and_the_listener_hears_each_entry() {
    let dir = tempfile::tempdir().unwrap();
    let heard = Arc::new(Mutex::new(Vec::new()));
    let sink = heard.clone();
    let log = Log::new(dir.path(), DEFAULT_MAX_BYTES, Some(Box::new(move |e| sink.lock().unwrap().push(e.message.clone()))));
    log.write("error", "a", "one");
    log.write("error", "a", "two");
    assert_eq!(*heard.lock().unwrap(), ["one", "two"]);

    log.clear().unwrap();
    assert!(log.read(10).is_empty());
    log.clear().unwrap(); // nothing left to remove is fine
}

#[test]
fn the_limit_is_kept_in_range_and_lowering_it_trims_the_files() {
    let dir = tempfile::tempdir().unwrap();
    let log = Log::new(dir.path(), DEFAULT_MAX_BYTES, None);
    assert_eq!(log.set_max_bytes(1), MIN_MAX_BYTES);
    assert_eq!(log.set_max_bytes(u64::MAX), MAX_MAX_BYTES);

    log.set_max_bytes(MAX_MAX_BYTES);
    let line = format!("{}\n", "z".repeat(99));
    fs::write(dir.path().join(OLD_FILE_NAME), line.repeat(4000)).unwrap(); // 400 kB
    fs::write(dir.path().join(FILE_NAME), line.repeat(4000)).unwrap();

    log.set_max_bytes(MIN_MAX_BYTES);
    let size = |name: &str| fs::metadata(dir.path().join(name)).map(|m| m.len()).unwrap_or(0);
    assert!(size(FILE_NAME) + size(OLD_FILE_NAME) <= MIN_MAX_BYTES);
    // What is kept starts at a whole line.
    assert!(fs::read_to_string(dir.path().join(OLD_FILE_NAME)).unwrap().starts_with('z'));
    assert_eq!(fs::read_to_string(dir.path().join(OLD_FILE_NAME)).unwrap().len() % 100, 0);
}

#[test]
fn a_capture_keeps_everything_from_start_to_stop_and_survives_a_restart() {
    use crate::applog::CAPTURE_RAW;
    let dir = tempfile::tempdir().unwrap();
    let log = Log::new(dir.path(), DEFAULT_MAX_BYTES, None);

    log.write("debug", "ui", "before the capture: dropped");
    log.start_capture().unwrap();
    log.write("debug", "get_thread", "ok in 3 ms");
    log.write("error", "sync", "it broke");

    // The app starts again while the capture runs: it carries on.
    let log = Log::new(dir.path(), DEFAULT_MAX_BYTES, None);
    assert!(log.capturing());
    log.write("debug", "page", "/settings");

    let captured = log.stop_capture().unwrap();
    let messages: Vec<_> = captured.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(messages, ["Debug capture started", "ok in 3 ms", "it broke", "/settings", "Debug capture stopped"]);
    assert!(!dir.path().join(CAPTURE_RAW).exists());
    assert!(log.stop_capture().is_err());

    // The log itself never keeps debug entries.
    let kept: Vec<_> = log.read(100).into_iter().map(|e| e.message).collect();
    assert_eq!(kept, ["Debug capture started", "it broke", "Debug capture stopped"]);
}
