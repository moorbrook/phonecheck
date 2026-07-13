#!/bin/sh
# Build the SpeechAnalyzer transcription helper (native/speech_helper).
# Requires macOS 26+ and the Xcode command line tools (swiftc).
# Invoked automatically by build.rs; safe to run manually.
set -eu

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$REPO_ROOT/native/speech_helper.swift"
OUT="$REPO_ROOT/native/speech_helper"

if ! command -v swiftc >/dev/null 2>&1; then
    echo "error: swiftc not found. Install the Xcode command line tools: xcode-select --install" >&2
    exit 1
fi

# Skip recompilation when the binary is already newer than the source.
if [ -x "$OUT" ] && [ "$OUT" -nt "$SRC" ]; then
    exit 0
fi

swiftc -O -parse-as-library "$SRC" -o "$OUT"
echo "built $OUT"
