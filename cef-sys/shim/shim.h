// shim.h — C ABI between the Rust shell and the CEF (Chromium content API)
// embedding. Frames cross as raw BGRA pixels + damage rects; input, focus and
// device metrics travel the other way. No PNG, no base64, no screenshots.
#ifndef STRIP_CEF_SHIM_H
#define STRIP_CEF_SHIM_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

