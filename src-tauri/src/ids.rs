use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use chrono::{DateTime, Utc};

const BASE36: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

static COUNTER: AtomicU64 = AtomicU64::new(0);
static LAST_MILLIS: AtomicI64 = AtomicI64::new(0);

/// Lower-case ASCII slug; anything else collapses to "-". Falls back to "thread".
pub fn slugify(input: &str) -> String {
    slugify_as(input, "thread")
}

/// Like [`slugify`], with a different fallback for names that have no letters or digits in them.
pub fn slugify_as(input: &str, fallback: &str) -> String {
    let mut slug = String::new();
    let mut pending_dash = false;
    for c in input.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.push(c);
        } else {
            pending_dash = true;
        }
    }
    if slug.is_empty() { fallback.to_string() } else { slug }
}

/// An id together with the moment it stands for. Both come from one strictly increasing millisecond clock, so
/// things created in quick succession (a document type's seeded questions, say) keep their order both in memory
/// and after being read back from files, where ids and timestamps are all there is to sort by.
pub struct Stamp {
    pub id: String,
    pub at: String,
}

pub fn new_stamp() -> Stamp {
    let millis = next_millis();
    Stamp { id: format!("{millis}-{}", random_string(8)), at: format_millis(millis) }
}

/// "<epoch-millis>-<8 random chars>": ids sort chronologically as strings.
pub fn new_id() -> String {
    new_stamp().id
}

/// Timestamps are stored and compared as ISO-8601 strings in UTC with millisecond precision
/// (`2026-10-16T09:18:22.123Z`).
pub fn now() -> String {
    format_millis(next_millis())
}

pub fn format_millis(millis: i64) -> String {
    DateTime::from_timestamp_millis(millis)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// The wall clock, nudged forward when two calls land in the same millisecond.
fn next_millis() -> i64 {
    let now = Utc::now().timestamp_millis();
    let mut previous = LAST_MILLIS.load(Ordering::Relaxed);
    loop {
        let next = now.max(previous + 1);
        match LAST_MILLIS.compare_exchange_weak(previous, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(actual) => previous = actual,
        }
    }
}

/// Uniqueness matters here, not secrecy: a randomly keyed hasher fed a counter and the clock is plenty.
fn random_string(length: usize) -> String {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    hasher.write_i64(Utc::now().timestamp_nanos_opt().unwrap_or_default());
    let mut n = hasher.finish();

    let mut out = String::with_capacity(length);
    for _ in 0..length {
        out.push(BASE36[(n % 36) as usize] as char);
        n /= 36;
    }
    out
}
