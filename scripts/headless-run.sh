#!/usr/bin/env bash
# Headless verification harness: cage (headless wlroots) -> nested niri ->
# Strip Browser. Screenshots via grim against the headless cage output.
# Drives the browser through its control socket (same dispatch path as keys).
#
# Usage: scripts/headless-run.sh
set -euo pipefail

CAGE="$HOME/cage-root/root/usr/bin/cage"
export LD_LIBRARY_PATH="$HOME/cage-root/root/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export WLR_BACKENDS=headless
export WLR_LIBINPUT_NO_DEVICES=1
export WLR_RENDERER_ALLOW_SOFTWARE=1
