//! Build script: compiles CEF's official libcef_dll_wrapper (the
//! battle-tested C++ -> C bridge that ships in every CEF binary dist) plus
//! our small C-ABI shim, with the exact compiler flags CEF's own cmake uses
//! on Linux (see vendor/cef/cmake/cef_variables.cmake). We do NOT run cmake:
//! the wrapper is a plain static library and cc gives us the -j control we
//! need (capped at 8 jobs via .cargo/config.toml).

use std::path::PathBuf;

fn cef_root() -> PathBuf {
    // vendored CEF distribution; override with STRIP_CEF_ROOT when relocating.
    std::env::var("STRIP_CEF_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../vendor/cef"))
}

fn main() {
    let root = cef_root();
    let root = root.canonicalize().unwrap_or(root);
    let wrapper = root.join("libcef_dll");
    let inc = root.display().to_string();
    let release = std::env::var("PROFILE").as_deref() == Ok("release");

    println!("cargo:rerun-if-env-changed=STRIP_CEF_ROOT");
    println!("cargo:rerun-if-changed=shim/shim.cc");
    println!("cargo:rerun-if-changed=shim/shim.h");

    // -- flags mirror vendor/cef/cmake/cef_variables.cmake (OS_LINUX) --------
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .include(&inc)
        .flag("-std=c++20")
        .flag("-fno-exceptions")
        .flag("-fno-rtti")
        .flag("-fno-threadsafe-statics")
        .flag("-fvisibility=hidden")
        .flag("-fvisibility-inlines-hidden")
        .flag("-fno-strict-aliasing")
        .flag("-fPIC")
        .flag("-fstack-protector")
        .flag("-funwind-tables")
        .flag("--param=ssp-buffer-size=4")
        .flag("-pipe")
        .flag("-pthread")
        .flag("-Wno-missing-field-initializers")
        .flag("-Wno-unused-parameter")
        .flag("-Wno-sign-compare")
        .flag("-Wno-deprecated-declarations")
        .define("__STDC_CONSTANT_MACROS", None)
        .define("__STDC_FORMAT_MACROS", None)
        .define("_FILE_OFFSET_BITS", "64")
        // The wrapper talks to the exported C API of the shared libcef.
        .define("WRAPPING_CEF_SHARED", None);

    if release {
        build.opt_level(2).define("NDEBUG", None);
    } else {
        build.opt_level(0);
    }

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let dmabuf_xml = "/usr/share/wayland-protocols/stable/linux-dmabuf/linux-dmabuf-v1.xml";
    let viewporter_xml = "/usr/share/wayland-protocols/stable/viewporter/viewporter.xml";

    if std::path::Path::new(dmabuf_xml).exists() {
        let _ = std::process::Command::new("wayland-scanner")
            .args(["client-header", dmabuf_xml, out_dir.join("linux-dmabuf-v1-client-protocol.h").to_str().unwrap()])
            .status();
        let _ = std::process::Command::new("wayland-scanner")
            .args(["private-code", dmabuf_xml, out_dir.join("linux-dmabuf-v1-protocol.c").to_str().unwrap()])
            .status();
        build.define("STRIP_WAYLAND_DMABUF", "1");
    }

    if std::path::Path::new(viewporter_xml).exists() {
        let _ = std::process::Command::new("wayland-scanner")
            .args(["client-header", viewporter_xml, out_dir.join("viewporter-client-protocol.h").to_str().unwrap()])
            .status();
        let _ = std::process::Command::new("wayland-scanner")
            .args(["private-code", viewporter_xml, out_dir.join("viewporter-protocol.c").to_str().unwrap()])
            .status();
        build.define("STRIP_WAYLAND_VIEWPORTER", "1");
    }

    build.include(&out_dir);

    // Every wrapper translation unit + the shim.
    let mut files: Vec<PathBuf> = Vec::new();
    collect_sources(&wrapper, &mut files);
    files.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shim/shim.cc"));
    for f in &files {
        build.file(f);
    }
    build.compile("cef_shim");

    println!("cargo:rustc-link-lib=dylib=wayland-client");

    // Link the shared engine.
    let release_dir = root.join("Release");
    println!("cargo:rustc-link-search=native={}", release_dir.display());
    println!("cargo:rustc-link-lib=dylib=cef");

    // Runtime: resolve libcef.so relative to the binary (target/{debug,release}).
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../../vendor/cef/Release");
}

fn collect_sources(dir: &PathBuf, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("cc") {
            out.push(path);
        }
    }
}
