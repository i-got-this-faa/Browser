//! Audit tracing: real span logging to a JSONL file, gated behind
//! `STRIP_TRACE=/path/trace.jsonl`.
//!
//! Emits one JSON object per line:
//!   {"t":<unix_us>,"ph":"X","name":"frame","dur_us":<n>,"tid":"<thread>"}
//!   {"t":<unix_us>,"ph":"E","name":"frame.paint","fields":{...}}
//!
//! Span durations are wall-clock from span creation (`new_span`) to close
//! (`on_close`), which for our spans equals the held-scope duration because
//! spans are created exactly when their scope starts and dropped at its end.
//! Zero cost when `STRIP_TRACE` is unset: the global default subscriber is a
//! no-op and disabled callsites short-circuit.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write as _;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::span::{Attributes, Record};
use tracing::{Event, Id, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

struct JsonlLayer {
    file: Option<Mutex<File>>,
    /// Span id -> (static name, creation timestamp in microseconds).
    spans: Mutex<HashMap<u64, (&'static str, u64)>>,
}

fn thread_label() -> String {
    std::thread::current().name().unwrap_or("unnamed").to_string()
}

fn unix_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

fn write_line(file: &Mutex<File>, line: &str) {
    if let Ok(mut g) = file.lock() {
        let _ = g.write_all(line.as_bytes());
    }
}

