#!/usr/bin/env bash
# Verify a silent control-socket client cannot freeze the browser: connect,
# send nothing, and confirm the socket still answers a second client.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
