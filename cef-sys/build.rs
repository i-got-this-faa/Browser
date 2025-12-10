//! Build script: compiles CEF's official libcef_dll_wrapper (the
//! battle-tested C++ -> C bridge that ships in every CEF binary dist) plus
//! our small C-ABI shim, with the exact compiler flags CEF's own cmake uses
//! on Linux (see vendor/cef/cmake/cef_variables.cmake). We do NOT run cmake:
//! the wrapper is a plain static library and cc gives us the -j control we
//! need (capped at 8 jobs via .cargo/config.toml).

use std::path::PathBuf;

