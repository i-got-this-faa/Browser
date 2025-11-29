//! Link-time glue for the final binary.
//!
//! `cef-sys`'s own `cargo:rustc-link-arg` only reaches *its* targets, not
//! dependents, so the browser binary never received the libcef rpath. Emit it
//! here, where it applies to this binary's link step.
//!
//! `$ORIGIN/../../vendor/cef/Release` resolves from `target/{debug,release}`
//! back into the workspace's vendored CEF distribution, so the tree stays
//! relocatable. `STRIP_CEF_ROOT` (absolute) is used verbatim when set.

