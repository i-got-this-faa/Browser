//! Shared perf instrumentation macros.
//!
//! `perf_event!` emits a named instant event with fields; `perf_span!` either
//! returns a span handle you hold for the duration (`let _s = perf_span!("x")`)
//! or wraps a block. Durations are recorded by the JsonlLayer at span close.
//! Everything is emitted at TRACE level on the "perf" target, so without a
//! subscriber installed these compile to a thread-local enabled check.

/// Emit a named instant perf event with optional `key = value` fields.
#[macro_export]
macro_rules! perf_event {
    ($name:expr $(, $field:expr => $value:expr)* $(,)?) => {{
        ::tracing::event!(
            target: "perf",
            ::tracing::Level::TRACE,
            name = $name,
            $( $field = %$value, )*
        );
    }};
}

/// Span held for a scope: `let _s = perf_span!("frame");` (closes on drop),
/// or wrap a block: `perf_span!("frame", { ... })` which returns the block's
/// value.
#[macro_export]
macro_rules! perf_span {
    ($name:expr) => {{
        ::tracing::span!(target: "perf", ::tracing::Level::TRACE, $name)
    }};
    ($name:expr, $($body:tt)*) => {{
        let __scope = ::tracing::span!(target: "perf", ::tracing::Level::TRACE, $name);
        let __guard = __scope.enter();
        let __out = { $($body)* };
        drop(__guard);
        __out
    }};
}
