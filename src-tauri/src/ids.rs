use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;

const BASE36: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Lower-case ASCII slug; anything else collapses to "-". Falls back to "thread".
pub fn slugify(input: &str) -> String {
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
    if slug.is_empty() { "thread".to_string() } else { slug }
}

/// "<epoch-millis>-<8 random chars>": ids sort chronologically as strings.
pub fn new_id() -> String {
    format!("{}-{}", Utc::now().timestamp_millis(), random_string(8))
}

/// Timestamps are stored and compared as ISO-8601 strings in UTC with millisecond precision
/// (`2026-10-16T09:18:22.123Z`).
pub fn now() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
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
