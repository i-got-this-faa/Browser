//! Link-time glue for the final binary.
//!
//! `cef-sys`'s own `cargo:rustc-link-arg` only reaches *its* targets, not
//! dependents, so the browser binary never received the libcef rpath. Emit it
//! here, where it applies to this binary's link step.
//!
//! `$ORIGIN/../../vendor/cef/Release` resolves from `target/{debug,release}`
//! back into the workspace's vendored CEF distribution, so the tree stays
//! relocatable. `STRIP_CEF_ROOT` (absolute) is used verbatim when set.

fn main() {
    println!("cargo:rerun-if-env-changed=STRIP_CEF_ROOT");

    let rpath = match std::env::var("STRIP_CEF_ROOT") {
        // Absolute path: relocatable builds point elsewhere.
        Ok(root) if !root.is_empty() => format!("{root}/Release"),
        // Default layout: binary at target/<profile>/browser -> workspace root.
        _ => "$ORIGIN/../../vendor/cef/Release".to_string(),
    };

    println!("cargo:rustc-link-arg=-Wl,-rpath,{rpath}");
}
