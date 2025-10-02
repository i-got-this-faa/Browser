//! Shared perf instrumentation macros.
//!
//! `perf_event!` emits a named instant event with fields; `perf_span!` either
//! returns a span handle you hold for the duration (`let _s = perf_span!("x")`)
//! or wraps a block. Durations are recorded by the JsonlLayer at span close.
//! Everything is emitted at TRACE level on the "perf" target, so without a
//! subscriber installed these compile to a thread-local enabled check.

/// Emit a named instant perf event with optional `key = value` fields.
#[macro_export]
