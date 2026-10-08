//! Unix timestamps: the current one, and shown in local time.
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::TimeZone;

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The timestamp in local time, in a chrono `format`.
pub fn local_time(ts: i64, format: &str) -> String {
    chrono::Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|at| at.format(format).to_string())
        .unwrap_or_default()
}

/// `HH:MM` in local time.
pub fn clock(ts: i64) -> String {
    local_time(ts, "%H:%M")
}
