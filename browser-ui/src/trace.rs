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

impl<S> Layer<S> for JsonlLayer
where
    S: Subscriber,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, _ctx: Context<'_, S>) {
        let name = attrs.metadata().name();
        if let Ok(mut spans) = self.spans.lock() {
            spans.insert(id.into_u64(), (name, unix_us()));
        }
    }

    fn on_record(&self, _span: &Id, _values: &Record<'_>, _ctx: Context<'_, S>) {}

    fn on_enter(&self, _id: &Id, _ctx: Context<'_, S>) {}

    fn on_exit(&self, _id: &Id, _ctx: Context<'_, S>) {}

    fn on_close(&self, id: Id, _ctx: Context<'_, S>) {
        let entry = self
            .spans
            .lock()
            .ok()
            .and_then(|mut s| s.remove(&id.into_u64()));
        let Some((name, start_us)) = entry else { return };
        let Some(file) = self.file.as_ref() else { return };
        let dur = unix_us().saturating_sub(start_us);
        let line = format!(
            "{{\"t\":{start_us},\"ph\":\"X\",\"name\":\"{name}\",\"dur_us\":{dur},\"tid\":\"{}\"}}\n",
            thread_label(),
        );
        write_line(file, &line);
    }

    fn enabled(&self, meta: &tracing::Metadata<'_>, _ctx: Context<'_, S>) -> bool {
        // Only our own perf events/spans: other crates (zbus, etc.) also use
        // tracing and must not flood the audit trace.
        self.file.is_some() && meta.target() == "perf"
    }

