# SpeechAnalyzer Migration — 2026-07-13

Migrated phonecheck's transcription backend from whisper.cpp (whisper-rs +
bundled ggml-base.en.bin, 148 MB, cmake build dependency) to Apple's
SpeechAnalyzer / SpeechTranscriber API (macOS 26 Speech framework). The
Wav2Vec2 embedding pipeline (the actual greeting-similarity check) is
unchanged.

## Availability findings (Step 0)

Probed on macOS 26.5.1 (Apple Silicon, Xcode / Swift 6.3.3):

- SpeechTranscriber API present; 30 supported locales; en-US supported.
- The en-US on-device model was already installed system-wide
  (`SpeechTranscriber.installedLocales` included en-US, alongside 10 other
  locales — Siri/dictation had already provisioned it).
- `AssetInventory.status` reported `supported` before first use;
  `assetInstallationRequest` completed in 3.2 s (a per-process asset
  allocation/registration, not a model download — the model bytes were
  already on disk as an OS-managed system asset). Status after: `installed`.
- No app-managed download of any kind remains: the OS owns the model.

## Architecture

Subprocess helper, no Swift↔Rust FFI:

- `native/speech_helper.swift` (~90 lines) — WAV path in, JSON
  `{"transcript": "..."}` on stdout; errors on stderr with non-zero exit.
  Requests the OS-managed asset install if missing (one-time).
- `scripts/build_speech_helper.sh` — compiles it with `swiftc -O` to
  `native/speech_helper` (gitignored binary; skips recompile when fresh).
- `build.rs` — runs the script on macOS so `cargo build` yields a working
  setup with no extra steps.
- `src/speech.rs::transcribe_with_helper()` — writes 16 kHz mono i16 temp WAV
  (self-deleting guard), spawns the helper, bounded wait (60 s cap, 100 ms
  polls, kill on timeout), propagates stderr on non-zero exit, parses JSON.
- Config: `WHISPER_MODEL_PATH` → `SPEECH_HELPER_PATH`
  (default `./native/speech_helper`); `--validate` now checks the helper
  exists and points at the build script if not.
- Audio path unchanged: 8 kHz G.711 → rubato resample → 16 kHz f32 (needed by
  Wav2Vec2 anyway); the helper receives that as WAV. SpeechTranscriber accepts
  16 kHz files directly via AVAudioFile (verified end-to-end).

## Removed

- `whisper-rs` from Cargo.toml (and whisper.cpp / cmake from the build).
- All Whisper code from `src/model_manager.rs` (now Wav2Vec2-only) and
  `src/speech.rs`; `WHISPER_SAMPLE_RATE` renamed `TARGET_SAMPLE_RATE`.
- cmake/ggml/HuggingFace-download instructions from README, CLAUDE.md,
  .env.example (replaced by macOS 26+ / Xcode CLT requirement).
- `models/ggml-base.en.bin` is left on disk untouched but is no longer read
  by anything — delete it to reclaim 148 MB.
- Note: your local `.env` still has a `WHISPER_MODEL_PATH=` line; it is now
  silently ignored (the new `SPEECH_HELPER_PATH` defaults to
  `./native/speech_helper`). Safe to delete the line at your leisure.
  `./target/release/phonecheck --validate` passes as-is.

## Transcript comparison (test_audio.wav, 5.12 s fixture)

| Backend | Transcript |
|---|---|
| Whisper base.en (old) | "Thank you for calling Cubic Machinery. If you know your party's expected." |
| SpeechAnalyzer (new) | "Thank you for calling cubic machinery. If you know your parties." |

Both correct on the greeting; the fixture audio cuts off mid-sentence and
each engine handles the truncated tail differently (Whisper hallucinates
"expected", SpeechAnalyzer stops). New integration test
`tests/transcription.rs` runs the fixture through the real Rust → helper path
and pins the stable prefix ("thank you for calling cubic machinery").

## Test status

- `cargo build --release` — PASS (helper compiled by build.rs)
- `cargo test --release` — PASS (all suites, including new
  `tests/transcription.rs` end-to-end transcription of the fixture)
- `cargo clippy --all-targets` — PASS (0 errors; the repo's pre-existing
  warnings — unused imports/dead code in rtp, sip, embedding and test
  modules — are unchanged; no warnings in any file added or modified here
  beyond patterns that already existed, e.g. the same `as u64` cast in
  rtp/mod.rs under the old constant name)
- No test was weakened: config tests re-pinned key/default to the new
  SPEECH_HELPER_PATH; transcription coverage went from zero (never tested
  end-to-end before) to a real fixture test plus helper-missing /
  JSON-parsing / error-propagation unit tests.

## Commit

- `0b0830a` on master (not pushed): "speech: migrate transcription from
  whisper.cpp to Apple SpeechAnalyzer" — 18 files, +461/−318.
